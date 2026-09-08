//! Configuration validation.
//!
//! These exercise the rule that matters most for a physical surface: an
//! operator must be able to predict what a control does by reading the file,
//! and anything PushOS cannot predict is refused rather than guessed at.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use pushos_config::{ConfigFile, Problem, RuntimeConfig};
use pushos_domain::binding::BindingKey;
use pushos_domain::controls::{ButtonId, ControlId, PadIndex};
use pushos_domain::gesture::Gesture;
use pushos_domain::permissions::Permission;

fn parse(text: &str) -> ConfigFile {
    toml::from_str(text).expect("the test configuration is well-formed TOML")
}

fn build(text: &str) -> RuntimeConfig {
    RuntimeConfig::build(&parse(text)).expect("the test configuration should be valid")
}

fn problems(text: &str) -> Vec<Problem> {
    match RuntimeConfig::build(&parse(text)) {
        Ok(_) => panic!("the configuration should have been rejected"),
        Err(error) => match error {
            pushos_config::ConfigError::Invalid { problems } => problems,
            other => panic!("expected a validation failure, got {other}"),
        },
    }
}

const PAGES: &str = r#"
[[pages]]
id = "home"
name = "Home"

[[pages]]
id = "development"
name = "Development"

[[pages]]
id = "music"
name = "Music"
"#;

#[test]
fn a_complete_configuration_builds() {
    let config = build(&format!(
        r#"
        [runtime]
        home_page = "home"

        [gestures]
        hold_threshold_ms = 500
        double_tap_window_ms = 250

        [permissions]
        granted = ["media.control", "application.launch"]

        {PAGES}

        [[bindings]]
        control = "button.play"
        gesture = "press"
        action = "media.play_pause"

        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        page = "development"
        action = "page.show"
        target = "music"
        label = "Music"
        "#
    ));

    assert_eq!(config.pages.len(), 3);
    assert_eq!(config.bindings.len(), 2);
    assert!(config.permissions.allows(Permission::MediaControl));
    assert!(!config.permissions.allows(Permission::ShellExecute));
    assert_eq!(config.timing.hold_threshold.as_millis(), 500);
}

#[test]
fn nothing_is_permitted_unless_it_is_listed() {
    let config = build(PAGES);
    for permission in Permission::ALL {
        assert!(
            !config.permissions.allows(permission),
            "{permission} was granted without being configured"
        );
    }
}

#[test]
fn the_target_shorthand_becomes_a_parameter() {
    let config = build(&format!(
        r#"{PAGES}
        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        action = "page.show"
        target = "music"
        "#
    ));

    let key = BindingKey::new(
        ControlId::Pad(PadIndex::new(0).expect("0 is in range")),
        Gesture::Tap,
    );
    let binding = &config.bindings.candidates(key)[0];
    assert_eq!(binding.action.params.text("target"), Some("music"));
}

#[test]
fn an_unknown_control_names_the_binding_it_came_from() {
    let found = problems(&format!(
        r#"{PAGES}
        [[bindings]]
        control = "pad.99"
        gesture = "tap"
        action = "page.next"
        "#
    ));

    assert_eq!(found.len(), 1);
    assert!(matches!(found[0], Problem::Control { index: 1, .. }));
}

#[test]
fn every_problem_is_reported_not_just_the_first() {
    let found = problems(&format!(
        r#"{PAGES}
        [[bindings]]
        control = "pad.99"
        gesture = "tap"
        action = "page.next"

        [[bindings]]
        control = "pad.1"
        gesture = "wiggle"
        action = "page.next"

        [[bindings]]
        control = "pad.2"
        gesture = "tap"
        action = "notaselector"
        "#
    ));

    assert_eq!(
        found.len(),
        3,
        "one editing pass should be able to fix everything"
    );
    assert!(matches!(found[0], Problem::Control { .. }));
    assert!(matches!(found[1], Problem::Gesture { .. }));
    assert!(matches!(found[2], Problem::Action { .. }));
}

#[test]
fn a_binding_on_an_undeclared_page_is_refused() {
    let found = problems(&format!(
        r#"{PAGES}
        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        page = "nowhere"
        action = "page.next"
        "#
    ));

    assert!(matches!(found[0], Problem::UnknownPage { index: 1, .. }));
}

#[test]
fn an_undeclared_home_page_is_refused() {
    let found = problems(&format!(
        r#"
        [runtime]
        home_page = "nowhere"
        {PAGES}
        "#
    ));

    assert!(matches!(found[0], Problem::UnknownHomePage { .. }));
}

#[test]
fn two_pages_with_one_identity_are_refused() {
    let found = problems(
        r#"
        [[pages]]
        id = "home"
        name = "Home"

        [[pages]]
        id = "home"
        name = "Home Again"
        "#,
    );

    assert!(matches!(found[0], Problem::DuplicatePage { .. }));
}

#[test]
fn a_declared_scope_that_contradicts_the_other_fields_is_refused() {
    let found = problems(&format!(
        r#"{PAGES}
        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        scope = "global"
        page = "development"
        action = "page.next"
        "#
    ));

    assert!(matches!(found[0], Problem::ScopeMismatch { index: 1, .. }));
}

#[test]
fn a_declared_scope_that_agrees_is_accepted() {
    let config = build(&format!(
        r#"{PAGES}
        [[bindings]]
        control = "button.play"
        gesture = "press"
        scope = "global"
        action = "media.play_pause"

        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        scope = "page"
        page = "development"
        action = "page.next"
        "#
    ));
    assert_eq!(config.bindings.len(), 2);
}

#[test]
fn two_bindings_precedence_cannot_separate_are_refused() {
    let found = problems(&format!(
        r#"{PAGES}
        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        action = "page.next"

        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        action = "page.previous"
        "#
    ));

    assert!(
        found
            .iter()
            .any(|problem| matches!(problem, Problem::Conflict(_))),
        "an ambiguous surface must be refused, not resolved arbitrarily"
    );
}

#[test]
fn the_same_control_on_different_pages_is_not_a_conflict() {
    let config = build(&format!(
        r#"{PAGES}
        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        page = "development"
        action = "page.next"

        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        page = "music"
        action = "page.previous"
        "#
    ));
    assert_eq!(config.bindings.len(), 2);
}

#[test]
fn differing_priority_separates_two_otherwise_identical_bindings() {
    let config = build(&format!(
        r#"{PAGES}
        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        action = "page.next"
        priority = 1

        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        action = "page.previous"
        priority = 2
        "#
    ));
    assert_eq!(config.bindings.len(), 2);
}

#[test]
fn two_bindings_claiming_one_identity_are_refused() {
    let found = problems(&format!(
        r#"{PAGES}
        [[bindings]]
        id = "shared"
        control = "pad.0"
        gesture = "tap"
        action = "page.next"

        [[bindings]]
        id = "shared"
        control = "pad.1"
        gesture = "tap"
        action = "page.previous"
        "#
    ));

    assert!(matches!(found[0], Problem::DuplicateBindingId { .. }));
}

#[test]
fn timings_that_would_make_gestures_ambiguous_are_refused() {
    let found = problems(
        "
        [gestures]
        hold_threshold_ms = 100
        double_tap_window_ms = 300
        ",
    );

    assert!(matches!(found[0], Problem::Timing(_)));
}

#[test]
fn a_hold_binding_registers_a_timer_interest() {
    let config = build(&format!(
        r#"{PAGES}
        [[bindings]]
        control = "button.record"
        gesture = "hold"
        action = "page.home"
        "#
    ));

    let interest = config.bindings.gesture_interest();
    assert!(interest.wants_hold(ControlId::Button(ButtonId::Record)));
    assert!(!interest.wants_double_tap(ControlId::Button(ButtonId::Record)));
}

#[test]
fn pages_step_forward_and_backward_with_wrapping() {
    let config = build(PAGES);
    let home = config.pages[0].id.clone();
    let music = config.pages[2].id.clone();

    assert_eq!(
        config.page_after(Some(&home)).map(|p| p.id.as_str()),
        Some("development")
    );
    assert_eq!(
        config.page_after(Some(&music)).map(|p| p.id.as_str()),
        Some("home")
    );
    assert_eq!(
        config.page_before(Some(&home)).map(|p| p.id.as_str()),
        Some("music")
    );
    assert_eq!(config.page_position(&home), Some((1, 3)));
}

#[test]
fn stepping_pages_with_none_configured_yields_nothing() {
    let config = build("");
    assert!(config.page_after(None).is_none());
    assert!(config.page_before(None).is_none());
}

// --- Agents and providers ----------------------------------------------------

const AGENTS: &str = r#"
[[providers]]
id = "claude"
program = "npx"
args = ["-y", "@agentclientprotocol/claude-agent-acp@latest"]

[[providers]]
id = "codex"
program = "npx"
args = ["-y", "@agentclientprotocol/codex-acp@latest"]

[[agents]]
id = "builder"
name = "Builder"
objective = "Implement approved work safely and concisely."
preferred = ["claude", "codex"]
permissions = ["shell.execute", "git.commit"]

[[agents]]
id = "reviewer"
name = "Reviewer"
preferred = ["codex"]
"#;

#[test]
fn agent_roles_and_providers_build() {
    let config = build(AGENTS);

    assert_eq!(config.providers.len(), 2);
    assert_eq!(config.agents.len(), 2);

    let builder = config
        .agent(&pushos_domain::ids::AgentId::new("builder"))
        .expect("the builder is declared");
    assert_eq!(builder.name, "Builder");
    assert!(builder.objective.contains("Implement"));
    assert!(builder.permissions.allows(Permission::ShellExecute));
    assert!(!builder.permissions.allows(Permission::DeployProduction));
}

#[test]
fn a_role_naming_a_provider_that_is_not_declared_is_refused() {
    let found = problems(
        r#"
        [[providers]]
        id = "claude"
        program = "npx"

        [[agents]]
        id = "builder"
        name = "Builder"
        preferred = ["cursor"]
        "#,
    );

    assert!(
        found
            .iter()
            .any(|problem| matches!(problem, Problem::UnknownAgentProvider { .. })),
        "a typo in a provider name must not silently fall back"
    );
}

#[test]
fn two_roles_with_one_identity_are_refused() {
    let found = problems(
        r#"
        [[agents]]
        id = "builder"
        name = "Builder"

        [[agents]]
        id = "builder"
        name = "Other Builder"
        "#,
    );
    assert!(
        found
            .iter()
            .any(|problem| matches!(problem, Problem::DuplicateAgent { .. }))
    );
}

#[test]
fn two_providers_with_one_name_are_refused() {
    let found = problems(
        r#"
        [[providers]]
        id = "claude"
        program = "npx"

        [[providers]]
        id = "claude"
        program = "something-else"
        "#,
    );
    assert!(
        found
            .iter()
            .any(|problem| matches!(problem, Problem::DuplicateProvider { .. }))
    );
}

#[test]
fn a_provider_with_nothing_to_run_is_refused() {
    let found = problems(
        r#"
        [[providers]]
        id = "claude"
        program = "  "
        "#,
    );
    assert!(
        found
            .iter()
            .any(|problem| matches!(problem, Problem::ProviderWithoutProgram { .. }))
    );
}

#[test]
fn a_role_expressing_no_preference_is_accepted() {
    let config = build(
        r#"
        [[providers]]
        id = "claude"
        program = "npx"

        [[agents]]
        id = "builder"
        name = "Builder"
        "#,
    );

    let builder = config
        .agent(&pushos_domain::ids::AgentId::new("builder"))
        .expect("declared");
    assert!(
        builder.preferred.is_empty(),
        "no preference means any provider will do"
    );
}

#[test]
fn a_role_is_granted_nothing_it_did_not_ask_for() {
    let config = build(
        r#"
        [[agents]]
        id = "reader"
        name = "Reader"
        permissions = ["filesystem.read"]
        "#,
    );

    let reader = config
        .agent(&pushos_domain::ids::AgentId::new("reader"))
        .expect("declared");
    assert!(reader.permissions.allows(Permission::FilesystemRead));
    for denied in [
        Permission::ShellExecute,
        Permission::GitPush,
        Permission::FilesystemWrite,
    ] {
        assert!(
            !reader.permissions.allows(denied),
            "{denied:?} was not asked for"
        );
    }
}

#[test]
fn bindings_can_name_a_role_as_their_target() {
    let config = build(&format!(
        r#"{AGENTS}
        {PAGES}

        [[bindings]]
        control = "pad.16"
        gesture = "hold"
        page = "development"
        action = "agent.prompt"
        target = "workspace:sydclaw/role:builder"
        label = "Talk to Builder"
        "#
    ));

    let key = BindingKey::new(
        ControlId::Pad(PadIndex::new(16).expect("in range")),
        Gesture::Hold,
    );
    let binding = &config.bindings.candidates(key)[0];
    assert_eq!(binding.action.selector.to_string(), "agent.prompt");

    let target: pushos_domain::agent::AgentTarget = binding
        .action
        .params
        .text("target")
        .expect("the target is present")
        .parse()
        .expect("and it parses");
    assert_eq!(
        target,
        pushos_domain::agent::AgentTarget::role("builder").in_workspace("sydclaw")
    );
}
