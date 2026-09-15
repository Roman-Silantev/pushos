//! Working out what the lights should say.
//!
//! A pad that does something is lit; a pad that does nothing is dark. That is
//! the whole contract, and it is derived from the same binding table the
//! resolver uses, so the surface can never advertise a binding that is not
//! there.
//!
//! A pad that stands for something with a state of its own shows that state
//! instead. Eight pads for eight coding sessions, all the same colour, tell an
//! operator nothing they did not already know; the one that is stuck on a
//! question needs to be the one that looks different. The same goes for a pad
//! that starts a role: while its agent works, waits or has failed, the pad says
//! which.

use pushos_config::RuntimeConfig;
use pushos_domain::agent::{AgentState, AgentTarget};
use pushos_domain::attached::{Activity, Attached, AttachedTarget};
use pushos_domain::binding::BindingKey;
use pushos_domain::color::{LedState, StatusColor};
use pushos_domain::context::SurfaceContext;
use pushos_domain::controls::{ControlId, PadIndex};
use pushos_domain::gesture::Gesture;
use pushos_ui::LedPlan;

use super::RoleActivity;

/// The gestures a control can carry that make it worth lighting.
///
/// A control bound only to `release` is not advertised: the operator has
/// nothing to aim at until they have already pressed it.
const ADVERTISED: [Gesture; 5] = [
    Gesture::Tap,
    Gesture::DoubleTap,
    Gesture::Hold,
    Gesture::Press,
    Gesture::ShiftHold,
];

/// Everything with a state of its own that a light can stand for.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Showing<'a> {
    /// The sessions PushOS did not start, in the order they were found.
    pub(crate) sessions: &'a [Attached],
    /// Where the bank of eight in view starts.
    pub(crate) from: usize,
    /// What each role's agent is doing, most recent session first.
    pub(crate) roles: &'a [RoleActivity],
}

/// Builds the light plan for the current surface.
pub(crate) fn plan_showing(
    config: &RuntimeConfig,
    context: &SurfaceContext,
    showing: &Showing<'_>,
) -> LedPlan {
    let mut plan = LedPlan::new();

    for pad in PadIndex::all() {
        let control = ControlId::Pad(pad);
        plan.set(control, light_for(config, context, control, showing));
    }

    for control in ControlId::all().filter(|control| matches!(control, ControlId::Button(_))) {
        plan.set(control, light_for(config, context, control, showing));
    }

    plan
}

/// The namespace whose bindings stand for a session rather than an instruction.
const SESSIONS: &str = "session";

/// The namespace whose bindings name a role.
const AGENTS: &str = "agent";

/// What a control stands for, as far as sessions go.
enum StandsFor {
    /// It is not a session's control.
    Nothing,
    /// It names a session, and nothing is there: an empty slot, or a window
    /// that has been closed.
    Absent,
    /// It names a session that is open, doing this.
    Session(Activity),
}

/// What a control stands for, when it stands for a session.
///
/// Read from the binding itself rather than configured twice: a pad bound to
/// `session.show` with a target already says which session it is, and asking
/// the operator to say so again would be a second place to get it wrong.
fn session_on(
    config: &RuntimeConfig,
    context: &SurfaceContext,
    control: ControlId,
    sessions: &[Attached],
    from: usize,
) -> StandsFor {
    let target: Option<AttachedTarget> = ADVERTISED
        .iter()
        .flat_map(|gesture| {
            config
                .bindings
                .candidates(BindingKey::new(control, *gesture))
        })
        .find(|binding| {
            binding.scope.applies_to(context)
                && binding.action.selector.provider.as_str() == SESSIONS
        })
        .and_then(|binding| binding.action.params.text("target"))
        .and_then(|written| written.parse().ok());
    let Some(target) = target else {
        return StandsFor::Nothing;
    };

    // A slot is a position in the bank being shown, so it is resolved the same
    // way the provider resolves it. Anything else answers for itself.
    let open = if let AttachedTarget::Slot(at) = target {
        sessions.get(from + usize::from(at) - 1)
    } else {
        sessions.iter().find(|session| target.matches(session))
    };

    open.map_or(StandsFor::Absent, |session| {
        StandsFor::Session(session.activity)
    })
}

/// What the agent filling the role a control names is doing.
///
/// Read from the binding, like a session's: a pad that starts `role:markets`
/// already says which role it is. The project is the binding's own when it
/// names one, and the one in effect otherwise, which is how the pad resolves
/// the role when it is pressed.
fn role_on(
    config: &RuntimeConfig,
    context: &SurfaceContext,
    control: ControlId,
    roles: &[RoleActivity],
) -> Option<AgentState> {
    let target: AgentTarget = ADVERTISED
        .iter()
        .flat_map(|gesture| {
            config
                .bindings
                .candidates(BindingKey::new(control, *gesture))
        })
        .find(|binding| {
            binding.scope.applies_to(context) && binding.action.selector.provider.as_str() == AGENTS
        })
        .and_then(|binding| binding.action.params.text("target"))
        .and_then(|written| written.parse().ok())?;

    let AgentTarget::Role { agent, workspace } = target else {
        return None;
    };
    let workspace = workspace.or_else(|| context.workspace.clone());
    RoleActivity::state_of(roles, &agent, workspace.as_ref())
}

fn light_for(
    config: &RuntimeConfig,
    context: &SurfaceContext,
    control: ControlId,
    showing: &Showing<'_>,
) -> LedState {
    // What a session is doing outranks the fact that a pad is bound: the pad
    // being bound is what the operator already knows. A session at rest, or a
    // slot with no session in it, is dark rather than glowing as merely bound,
    // so the lights that are on are the ones worth looking at.
    match session_on(config, context, control, showing.sessions, showing.from) {
        StandsFor::Session(activity) if activity.is_resting() => return LedState::OFF,
        StandsFor::Session(activity) => return LedState::from_status(activity.status_color()),
        StandsFor::Absent => return LedState::OFF,
        StandsFor::Nothing => {}
    }

    // A role's pad lights for what is worth looking at: working, waiting on
    // the operator, or failed. A role with nothing happening keeps the glow of
    // any bound pad, so fifty-six roles at rest are not fifty-six colours.
    if let Some(state) = role_on(config, context, control, showing.roles)
        && (state.is_busy() || state.needs_operator() || state == AgentState::Failed)
    {
        return LedState::from_status(state.status_color());
    }

    let bound = ADVERTISED.iter().any(|gesture| {
        config
            .bindings
            .candidates(BindingKey::new(control, *gesture))
            .iter()
            .any(|binding| binding.scope.applies_to(context))
    });

    if bound {
        LedState::from_status(StatusColor::Idle)
    } else {
        LedState::OFF
    }
}

#[cfg(test)]
mod tests {
    use pushos_config::ConfigFile;

    use super::*;

    fn config(text: &str) -> RuntimeConfig {
        let parsed: ConfigFile =
            toml::from_str(text).expect("the test configuration is well-formed");
        RuntimeConfig::build(&parsed).expect("the test configuration is valid")
    }

    fn pad(index: u8) -> ControlId {
        ControlId::Pad(PadIndex::new(index).expect("test pad index is in range"))
    }

    const PAGES: &str = r#"
        [[pages]]
        id = "home"
        name = "Home"

        [[pages]]
        id = "music"
        name = "Music"
    "#;

    #[test]
    fn a_bound_pad_is_lit_and_an_unbound_one_is_dark() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "pad.0"
            gesture = "tap"
            action = "page.next"
            "#
        ));

        let plan = plan_showing(&config, &SurfaceContext::empty(), &Showing::default());
        let lit: Vec<_> = plan
            .states()
            .iter()
            .filter(|(_, state)| *state != LedState::OFF)
            .map(|(control, _)| *control)
            .collect();

        assert_eq!(lit, [pad(0)]);
    }

    #[test]
    fn a_pad_bound_on_another_page_is_dark_here() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "pad.0"
            gesture = "tap"
            page = "music"
            action = "page.next"
            "#
        ));

        let on_home = plan_showing(
            &config,
            &SurfaceContext::empty().on_page("home"),
            &Showing::default(),
        );
        let on_music = plan_showing(
            &config,
            &SurfaceContext::empty().on_page("music"),
            &Showing::default(),
        );

        assert_eq!(
            on_home
                .states()
                .iter()
                .find(|(c, _)| *c == pad(0))
                .map(|(_, s)| *s),
            Some(LedState::OFF)
        );
        assert_ne!(
            on_music
                .states()
                .iter()
                .find(|(c, _)| *c == pad(0))
                .map(|(_, s)| *s),
            Some(LedState::OFF)
        );
    }

    #[test]
    fn a_control_bound_only_to_release_is_not_advertised() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "pad.5"
            gesture = "release"
            action = "page.next"
            "#
        ));

        let plan = plan_showing(&config, &SurfaceContext::empty(), &Showing::default());
        assert!(
            plan.states()
                .iter()
                .all(|(_, state)| *state == LedState::OFF)
        );
    }

    #[test]
    fn a_plan_covers_every_pad_and_button_and_nothing_else() {
        let plan = plan_showing(
            &config(PAGES),
            &SurfaceContext::empty(),
            &Showing::default(),
        );
        assert!(
            plan.states()
                .iter()
                .all(|(control, _)| control.is_illuminated())
        );
        assert_eq!(
            plan.states().len(),
            64 + pushos_domain::controls::ALL_BUTTONS.len()
        );
    }

    #[test]
    fn moving_page_only_changes_the_lights_that_differ() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "pad.0"
            gesture = "tap"
            action = "page.next"

            [[bindings]]
            control = "pad.1"
            gesture = "tap"
            page = "music"
            action = "page.previous"
            "#
        ));

        let on_home = plan_showing(
            &config,
            &SurfaceContext::empty().on_page("home"),
            &Showing::default(),
        );
        let on_music = plan_showing(
            &config,
            &SurfaceContext::empty().on_page("music"),
            &Showing::default(),
        );

        let changes = on_music.changes_from(&on_home);
        assert_eq!(changes.len(), 1, "only pad 1 differs between the two pages");
        assert_eq!(changes[0].0, pad(1));
    }

    /// Two session pads, as the sessions preset binds them.
    const SESSION_PADS: &str = r#"
        [[pages]]
        id = "sessions"
        name = "Sessions"

        [[bindings]]
        control = "pad.56"
        gesture = "tap"
        action = "session.show"
        target = "slot:1"

        [[bindings]]
        control = "pad.57"
        gesture = "tap"
        action = "session.show"
        target = "slot:2"

        [[bindings]]
        control = "pad.0"
        gesture = "tap"
        action = "page.next"
    "#;

    fn doing(activity: Activity) -> Attached {
        Attached::new("/dev/ttys001", "Sprint 2 setup", activity, "Terminal")
    }

    fn light(plan: &LedPlan, control: ControlId) -> LedState {
        plan.states()
            .iter()
            .find(|(candidate, _)| *candidate == control)
            .map_or(LedState::OFF, |(_, state)| *state)
    }

    #[test]
    fn a_session_with_nothing_happening_leaves_its_pad_dark() {
        // Not green. A surface left on all day should light what is going on
        // and who is wanted, not every session for being there.
        let config = config(SESSION_PADS);
        for resting in [Activity::Ready, Activity::Quiet] {
            let plan = plan_showing(
                &config,
                &SurfaceContext::empty(),
                &Showing {
                    sessions: &[doing(resting)],
                    from: 0,
                    roles: &[],
                },
            );
            assert_eq!(light(&plan, pad(56)), LedState::OFF, "{resting:?}");
        }
    }

    #[test]
    fn a_session_that_is_doing_something_or_wants_someone_is_lit() {
        let config = config(SESSION_PADS);
        for busy in [
            Activity::Working,
            Activity::NeedsDecision,
            Activity::Drafting,
        ] {
            let plan = plan_showing(
                &config,
                &SurfaceContext::empty(),
                &Showing {
                    sessions: &[doing(busy)],
                    from: 0,
                    roles: &[],
                },
            );
            assert_eq!(
                light(&plan, pad(56)),
                LedState::from_status(busy.status_color()),
                "{busy:?}"
            );
        }
    }

    #[test]
    fn a_slot_with_no_session_in_it_is_dark_rather_than_glowing_as_bound() {
        // One session open, two slots bound: the second has nothing to show.
        let config = config(SESSION_PADS);
        let plan = plan_showing(
            &config,
            &SurfaceContext::empty(),
            &Showing {
                sessions: &[doing(Activity::Working)],
                from: 0,
                roles: &[],
            },
        );
        assert_eq!(light(&plan, pad(57)), LedState::OFF);
    }

    #[test]
    fn a_control_that_is_not_a_sessions_still_glows_to_say_it_does_something() {
        let config = config(SESSION_PADS);
        let plan = plan_showing(&config, &SurfaceContext::empty(), &Showing::default());
        assert_eq!(
            light(&plan, pad(0)),
            LedState::from_status(StatusColor::Idle)
        );
    }

    const ROLE_PADS: &str = r#"
        [[agents]]
        id = "markets"
        name = "Markets"

        [[agents]]
        id = "analyst"
        name = "Analyst"

        [[bindings]]
        control = "pad.32"
        gesture = "tap"
        action = "agent.start"
        target = "role:markets"

        [[bindings]]
        control = "pad.32"
        gesture = "hold"
        action = "agent.select"
        target = "role:markets"

        [[bindings]]
        control = "pad.33"
        gesture = "tap"
        action = "agent.start"
        target = "role:analyst"
    "#;

    fn role(agent: &str, state: AgentState) -> RoleActivity {
        RoleActivity {
            agent: pushos_domain::ids::AgentId::new(agent),
            workspace: None,
            state,
        }
    }

    fn showing_roles(config: &RuntimeConfig, roles: &[RoleActivity]) -> LedPlan {
        plan_showing(
            config,
            &SurfaceContext::empty(),
            &Showing {
                roles,
                ..Showing::default()
            },
        )
    }

    #[test]
    fn a_role_whose_agent_is_working_or_waiting_says_so_on_its_pad() {
        let config = config(ROLE_PADS);
        for state in [
            AgentState::Starting,
            AgentState::Working,
            AgentState::WaitingApproval,
            AgentState::WaitingInput,
            AgentState::Failed,
        ] {
            let plan = showing_roles(&config, &[role("markets", state)]);
            assert_eq!(
                light(&plan, pad(32)),
                LedState::from_status(state.status_color()),
                "{state:?}"
            );
        }
    }

    #[test]
    fn a_role_waiting_on_the_operator_is_a_light_that_wants_a_person() {
        // Which is what keeps it lit, and wakes the surface, when nothing else is.
        let config = config(ROLE_PADS);
        let plan = showing_roles(&config, &[role("markets", AgentState::WaitingApproval)]);
        assert!(light(&plan, pad(32)).wants_a_person());
    }

    #[test]
    fn a_role_with_nothing_happening_glows_like_any_bound_pad() {
        // Fifty-six roles at rest are not fifty-six colours.
        let config = config(ROLE_PADS);
        let bound = LedState::from_status(StatusColor::Idle);
        for state in [
            AgentState::Sleeping,
            AgentState::Completed,
            AgentState::Cancelled,
            AgentState::Offline,
        ] {
            let plan = showing_roles(&config, &[role("markets", state)]);
            assert_eq!(light(&plan, pad(32)), bound, "{state:?}");
        }
        let never_started = showing_roles(&config, &[]);
        assert_eq!(light(&never_started, pad(32)), bound);
    }

    #[test]
    fn each_role_pad_shows_its_own_role() {
        let config = config(ROLE_PADS);
        let plan = showing_roles(&config, &[role("analyst", AgentState::Working)]);
        assert_eq!(
            light(&plan, pad(33)),
            LedState::from_status(StatusColor::Working)
        );
        assert_eq!(
            light(&plan, pad(32)),
            LedState::from_status(StatusColor::Idle),
            "the markets pad is not the analyst's"
        );
    }
}
