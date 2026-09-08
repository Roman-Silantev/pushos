//! Assembling the host-specific half of PushOS.
//!
//! One place decides which concrete adapters back the domain's ports. Swapping
//! a host means changing this file and nothing else.

use std::sync::Arc;

use pushos_acp::{AcpBackend, AgentCommand};
use pushos_actions::providers::{agent, application, media, page, shell, shortcut};
use pushos_agents::{AgentRoster, AgentSupervisor};
use pushos_config::RuntimeConfig;
use pushos_domain::ports::{ActionProvider, AgentObserver, ProcessRunner};
use pushos_macos::{AppleScriptMedia, OpenLauncher, ShortcutsCli, SystemProcessRunner};
use tracing::{info, warn};

/// Builds the agent supervisor from configuration.
///
/// Returns `None` when nothing is configured, because a surface with no agent
/// roles should not carry an agent namespace that refuses every binding.
pub(crate) fn agents(
    config: &RuntimeConfig,
    observer: Arc<dyn AgentObserver>,
    workspace_root: &std::path::Path,
) -> Option<Arc<AgentSupervisor>> {
    if config.agents.is_empty() {
        return None;
    }

    let mut roster = AgentRoster::new();
    for definition in &config.agents {
        roster.define(definition.clone());
    }

    // A provider is a command. One named without a command uses the default
    // PushOS ships for that name, if it has one; naming it is still required,
    // because PushOS does not start a subprocess the operator never mentioned.
    let mut installed = 0;
    for entry in &config.providers {
        let Some(command) = command_for(entry) else {
            warn!(
                provider = %entry.id,
                "PushOS has no default command for this provider; give it a `program`"
            );
            continue;
        };

        info!(provider = %entry.id, command = %command.describe(), "agent provider available");
        roster.install(Arc::new(AcpBackend::new(
            entry.id.as_str(),
            command,
            Arc::clone(&observer),
        )));
        installed += 1;
    }

    if installed == 0 {
        warn!(
            roles = config.agents.len(),
            "agent roles are configured but no provider can run them"
        );
        return None;
    }

    Some(Arc::new(AgentSupervisor::new(
        Arc::new(roster),
        observer,
        workspace_root,
    )))
}

/// The command that starts a configured provider.
fn command_for(entry: &pushos_config::model::ProviderEntry) -> Option<AgentCommand> {
    let command = match &entry.program {
        Some(program) => AgentCommand::new(program.as_str(), entry.args.clone()),
        None => AgentCommand::known(&entry.id)?,
    };

    Some(entry.env.iter().fold(command, |command, (name, value)| {
        command.with_env(name, value)
    }))
}

/// Builds every action provider PushOS ships with.
///
/// A provider whose backend cannot be configured is left out rather than
/// installed in a broken state, so a binding that references it fails with
/// "no provider" instead of failing halfway through doing something.
pub(crate) fn providers(
    config: &RuntimeConfig,
    agents: Option<&Arc<AgentSupervisor>>,
) -> Vec<Arc<dyn ActionProvider>> {
    let processes: Arc<dyn ProcessRunner> = Arc::new(SystemProcessRunner::new());

    let mut providers: Vec<Arc<dyn ActionProvider>> = vec![
        Arc::new(page::PageProvider::new()),
        Arc::new(application::ApplicationProvider::new(Arc::new(
            OpenLauncher::new(Arc::clone(&processes)),
        ))),
        Arc::new(shortcut::ShortcutProvider::new(Arc::new(
            ShortcutsCli::new(Arc::clone(&processes)),
        ))),
        Arc::new(shell::ShellProvider::new(Arc::clone(&processes))),
    ];

    // Agents are offered only when there is something to run them, so a
    // binding fails with "no provider" rather than "unknown verb".
    if let Some(supervisor) = agents {
        providers.push(Arc::new(agent::AgentProvider::new(Arc::clone(supervisor))));
    }

    if let Some(controller) = media_controller(config, &processes) {
        providers.push(Arc::new(media::MediaProvider::new(controller)));
    } else {
        warn!("media actions are unavailable; check the configured media player");
    }

    providers
}

fn media_controller(
    config: &RuntimeConfig,
    processes: &Arc<dyn ProcessRunner>,
) -> Option<Arc<dyn pushos_domain::ports::MediaController>> {
    let controller = match &config.media_player {
        Some(player) => AppleScriptMedia::for_player(Arc::clone(processes), player.as_str())
            .inspect_err(|error| warn!(%error, "the configured media player is not usable"))
            .ok()?,
        None => AppleScriptMedia::new(Arc::clone(processes)),
    };
    Some(Arc::new(controller))
}

#[cfg(test)]
mod tests {
    use pushos_config::ConfigFile;

    use super::*;

    fn config(text: &str) -> RuntimeConfig {
        let parsed: ConfigFile = toml::from_str(text).expect("well-formed");
        RuntimeConfig::build(&parsed).expect("valid")
    }

    #[test]
    fn every_shipped_namespace_is_present_by_default() {
        let namespaces: Vec<_> = providers(&config(""), None)
            .iter()
            .map(|provider| provider.name().to_string())
            .collect();

        for expected in ["page", "app", "shortcut", "shell", "media"] {
            assert!(
                namespaces.contains(&expected.to_owned()),
                "`{expected}` is missing"
            );
        }
    }

    #[test]
    fn namespaces_are_unique_so_the_registry_will_accept_them_all() {
        let mut namespaces: Vec<_> = providers(&config(""), None)
            .iter()
            .map(|provider| provider.name().to_string())
            .collect();
        namespaces.sort();
        let total = namespaces.len();
        namespaces.dedup();
        assert_eq!(total, namespaces.len());
    }

    #[test]
    fn a_surface_with_no_agent_roles_carries_no_agent_namespace() {
        // A namespace that refuses every binding is worse than none: the
        // failure would say "unknown verb" rather than "no agents configured".
        let namespaces: Vec<_> = providers(&config(""), None)
            .iter()
            .map(|provider| provider.name().to_string())
            .collect();
        assert!(!namespaces.contains(&"agent".to_owned()));
    }

    #[test]
    fn naming_a_provider_pushos_knows_is_enough_to_start_it() {
        let named = config(
            r#"
            [[providers]]
            id = "claude"

            [[agents]]
            id = "builder"
            name = "Builder"
            preferred = ["claude"]
            "#,
        );

        let (reporter, _updates) = pushos_runtime::AgentReporter::new();
        let supervisor = agents(&named, Arc::new(reporter), std::path::Path::new("/tmp"))
            .expect("a name PushOS knows needs no command repeated");
        assert_eq!(supervisor.roster().providers().len(), 1);
    }

    #[test]
    fn roles_with_no_provider_at_all_leave_agents_unavailable() {
        // Better than installing a namespace that refuses every binding, and
        // better than starting a subprocess nobody asked for.
        let orphaned = config(
            r#"
            [[agents]]
            id = "builder"
            name = "Builder"
            "#,
        );

        let (reporter, _updates) = pushos_runtime::AgentReporter::new();
        assert!(agents(&orphaned, Arc::new(reporter), std::path::Path::new("/tmp")).is_none());
    }

    #[test]
    fn a_provider_pushos_does_not_know_and_that_names_no_command_is_skipped() {
        let unknown = config(
            r#"
            [[providers]]
            id = "something-else"

            [[agents]]
            id = "builder"
            name = "Builder"
            preferred = ["something-else"]
            "#,
        );

        let (reporter, _updates) = pushos_runtime::AgentReporter::new();
        assert!(agents(&unknown, Arc::new(reporter), std::path::Path::new("/tmp")).is_none());
    }

    #[test]
    fn a_role_preferring_something_pushos_does_not_know_is_left_unfilled() {
        // Configuration validation refuses an undeclared provider, so reaching
        // here means the operator declared it; PushOS has no default to guess.
        let with_roles = config(
            r#"
            [[providers]]
            id = "mine"
            program = "my-agent"

            [[agents]]
            id = "builder"
            name = "Builder"
            preferred = ["mine"]
            "#,
        );

        let (reporter, _updates) = pushos_runtime::AgentReporter::new();
        let supervisor = agents(
            &with_roles,
            Arc::new(reporter),
            std::path::Path::new("/tmp"),
        )
        .expect("the declared provider is installed as written");
        assert_eq!(supervisor.roster().providers().len(), 1);
    }

    #[test]
    fn an_unusable_media_player_leaves_the_namespace_unclaimed() {
        let unusable = config("[runtime]\nmedia_player = \"Music\\\" to quit\"\n");
        let namespaces: Vec<_> = providers(&unusable, None)
            .iter()
            .map(|provider| provider.name().to_string())
            .collect();

        assert!(
            !namespaces.contains(&"media".to_owned()),
            "a provider must not be installed in a broken state"
        );
        assert!(
            namespaces.contains(&"shell".to_owned()),
            "the rest still work"
        );
    }
}
