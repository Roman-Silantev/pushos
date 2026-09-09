//! Driving sessions the operator already had open.
//!
//! PushOS did not start these and does not own them, so this namespace is
//! deliberately narrower than the one for terminals PushOS runs. It can say
//! which one it is looking at, bring one to the front, type into one, and stop
//! one. It cannot tell you what any of them exited with, because it will never
//! find out.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionResult, ActionStatus, DisplayIntent};
use pushos_domain::attached::{Attached, AttachedTarget};
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::{ActionVerb, AttachedId, ProviderName};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::{
    ActionProvider, AttachError, AttachedSessions, Key, ProviderCapabilities,
};
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::watch;
use tracing::info;

/// The namespace this provider claims.
pub const NAMESPACE: &str = "session";

/// What an interrupt sends.
///
/// The control character, not the word: a session waiting at a prompt should
/// be interrupted rather than told about interrupting.
const INTERRUPT: &str = "\u{3}";

/// Turns gestures into instructions for sessions PushOS did not start.
#[derive(Debug)]
pub struct SessionProvider {
    sessions: Arc<dyn AttachedSessions>,
    /// Which one the operator is looking at.
    ///
    /// Published rather than only held, so the display and the lights follow it
    /// without asking. A binding with no target acts on this one, the way it
    /// does for agents and terminals.
    selected: watch::Sender<Option<AttachedId>>,
    /// How far back through what a session said the operator has scrolled.
    ///
    /// Lines from the bottom. Reset whenever they look at something else,
    /// because a scroll position belongs to the thing it was scrolling.
    scrolled: AtomicUsize,
}

impl SessionProvider {
    /// Builds the provider over whatever can see other terminals.
    pub fn new(sessions: Arc<dyn AttachedSessions>) -> Self {
        Self {
            sessions,
            selected: watch::channel(None).0,
            scrolled: AtomicUsize::new(0),
        }
    }

    /// Follows which session the operator is looking at.
    pub fn watch(&self) -> watch::Receiver<Option<AttachedId>> {
        self.selected.subscribe()
    }

    /// The one the operator is looking at, if any.
    pub fn selected(&self) -> Option<AttachedId> {
        self.selected.borrow().clone()
    }

    /// What can be seen right now.
    pub async fn discover(&self) -> Result<Vec<Attached>, AttachError> {
        self.sessions.discover().await
    }

    /// The thing that can see other terminals, for whoever has to poll it.
    pub fn watcher(&self) -> Arc<dyn AttachedSessions> {
        Arc::clone(&self.sessions)
    }

    /// The session an action acts on.
    ///
    /// Named exactly, named by part of its title, or left out to mean the one
    /// most recently selected. One answer pad then serves every session, the
    /// way one serves every agent.
    async fn resolve(&self, context: &ActionContext) -> Result<Attached, ActionError> {
        let written = context.params().text("target").unwrap_or("selected");
        let target: AttachedTarget =
            written
                .parse()
                .map_err(|error: pushos_domain::attached::MalformedTarget| {
                    ActionError::backend(error.to_string(), ErrorClass::Validation, error)
                })?;

        let open = self.sessions.discover().await.map_err(into_action_error)?;

        if target == AttachedTarget::Selected {
            let held = self.selected();
            return held
                .and_then(|id| open.into_iter().find(|session| session.id == id))
                .ok_or_else(|| invalid("no session is selected"));
        }

        open.into_iter()
            .find(|session| target.matches(session))
            .ok_or_else(|| {
                ActionError::backend(
                    format!("no open session answers to `{written}`"),
                    ErrorClass::Validation,
                    std::io::Error::other("no such session"),
                )
            })
    }

    /// Records which session the operator is looking at.
    ///
    /// Looking at something else starts its history at the bottom, because a
    /// scroll position belongs to the thing it was scrolling.
    fn select(&self, session: &Attached) {
        let moved = self.selected.borrow().as_ref() != Some(&session.id);
        self.selected.send_replace(Some(session.id.clone()));
        if moved {
            self.scrolled.store(0, Ordering::Relaxed);
            info!(session = %session.id, title = session.label(), "session selected");
        }
    }

    /// Moves the selection through the open sessions.
    ///
    /// Wraps, and starts at one end when nothing is selected, so an operator
    /// turning an encoder never has to have chosen something first.
    async fn step(&self, forward: bool) -> Result<Attached, ActionError> {
        let open = self.sessions.discover().await.map_err(into_action_error)?;
        if open.is_empty() {
            return Err(invalid("nothing is open"));
        }

        let here = self
            .selected()
            .and_then(|id| open.iter().position(|session| session.id == id));

        let next = match (here, forward) {
            (None, true) => 0,
            (None, false) => open.len() - 1,
            (Some(at), true) => (at + 1) % open.len(),
            (Some(at), false) => (at + open.len() - 1) % open.len(),
        };

        let session = open[next].clone();
        self.select(&session);
        Ok(session)
    }

    /// Moves through what a session said, and reports where that leaves it.
    fn scroll(&self, by: i64, most: usize) -> usize {
        let held = self.scrolled.load(Ordering::Relaxed);
        // Done in whole numbers throughout and clamped before it comes back,
        // so nothing here depends on how wide a pointer is.
        let moved = if by >= 0 {
            held.saturating_add(usize::try_from(by).unwrap_or(most))
        } else {
            held.saturating_sub(by.unsigned_abs().try_into().unwrap_or(usize::MAX))
        };

        let moved = moved.min(most);
        self.scrolled.store(moved, Ordering::Relaxed);
        moved
    }

    /// Builds the focused view of a session, at wherever it is scrolled to.
    async fn focused(&self, session: &Attached) -> Result<ActionResult, ActionError> {
        let seen = self
            .sessions
            .read(&session.id, READ_MOST)
            .await
            .map_err(into_action_error)?;

        let said = last_lines(&seen, SHOWN_LINES);
        let back = self.scrolled.load(Ordering::Relaxed).min(said.len());
        let shown = said[..said.len() - back].to_vec();

        Ok(ActionResult {
            status: ActionStatus::Completed,
            message: Some(session.label().to_owned()),
            // Focused rather than reported: an operator who tapped a pad to
            // read what a session is doing is reading, and a panel that
            // reverted under them after six seconds would be one they could
            // not use.
            display: Some(DisplayIntent::Focus {
                kind: if back == 0 {
                    session.device().to_owned()
                } else {
                    format!("{} +{back} back", session.device())
                },
                title: session.label().to_owned(),
                state: session.activity.describe().to_owned(),
                tone: session.activity.status_color(),
                lines: shown,
            }),
        })
    }
}

#[async_trait]
impl ActionProvider for SessionProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(
            [
                "select",
                "next",
                "previous",
                "show",
                "scroll",
                "focus",
                "send",
                "press",
                "interrupt",
            ]
            .map(ActionVerb::new),
        )
        // Typing into a session runs whatever it makes of the keystrokes, which
        // is the same thing a terminal action does and needs the same saying
        // yes to. Looking at one needs nothing.
        .verb_requiring(ActionVerb::new("send"), [Permission::ShellExecute])
        .verb_requiring(ActionVerb::new("press"), [Permission::ShellExecute])
        .verb_requiring(ActionVerb::new("interrupt"), [Permission::ShellExecute])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        match context.definition.selector.verb.as_str() {
            "select" => {
                let session = self.resolve(&context).await?;
                self.select(&session);
                Ok(ActionResult {
                    status: ActionStatus::Completed,
                    message: Some(session.label().to_owned()),
                    display: Some(DisplayIntent::Toast {
                        title: session.label().to_owned(),
                        detail: Some(session.device().to_owned()),
                    }),
                })
            }

            "show" => {
                let session = self.resolve(&context).await?;
                self.select(&session);
                self.focused(&session).await
            }

            // Moving through the open sessions, for an encoder or an arrow.
            // Opening the one arrived at as well as selecting it, because
            // turning a knob to browse and seeing nothing change would be a
            // knob that does nothing.
            "next" | "previous" => {
                let forward = context.definition.selector.verb.as_str() == "next";
                let session = self.step(forward).await?;
                self.focused(&session).await
            }

            "scroll" => {
                let by = context
                    .params()
                    .get("by")
                    .and_then(pushos_domain::action::ParamValue::as_integer)
                    .unwrap_or(-1);

                let session = self.resolve(&context).await?;
                self.scroll(by, SHOWN_LINES);
                self.focused(&session).await
            }

            "focus" => {
                let session = self.resolve(&context).await?;
                self.select(&session);
                self.sessions
                    .focus(&session.id)
                    .await
                    .map_err(into_action_error)?;
                Ok(ActionResult {
                    status: ActionStatus::Completed,
                    message: Some(format!("{} at the front", session.label())),
                    display: None,
                })
            }

            "press" => {
                let key: Key = context.params().require_text("key")?.parse().map_err(
                    |error: pushos_domain::ports::UnknownKey| {
                        ActionError::backend(error.to_string(), ErrorClass::Validation, error)
                    },
                )?;

                let session = self.resolve(&context).await?;
                self.select(&session);
                self.sessions
                    .press(&session.id, key)
                    .await
                    .map_err(into_action_error)?;
                Ok(ActionResult {
                    status: ActionStatus::Completed,
                    message: Some(format!("{key} in {}", session.label())),
                    display: None,
                })
            }

            "send" | "interrupt" => {
                let session = self.resolve(&context).await?;
                let text = if context.definition.selector.verb.as_str() == "interrupt" {
                    INTERRUPT.to_owned()
                } else {
                    let written = context.params().require_text("text")?;
                    if written.is_empty() {
                        return Err(invalid("there is nothing to type"));
                    }
                    written.to_owned()
                };

                self.sessions
                    .send(&session.id, &text)
                    .await
                    .map_err(into_action_error)?;
                Ok(ActionResult {
                    status: ActionStatus::Completed,
                    message: Some(session.label().to_owned()),
                    display: None,
                })
            }

            other => Err(ActionError::UnknownVerb {
                provider: self.name(),
                verb: other.to_owned(),
            }),
        }
    }
}

/// How much of a session's screen to ask for.
///
/// More than a glance, because this is the view an operator opened on purpose
/// and the whole panel is theirs for it.
const READ_MOST: usize = 2_000;

/// How many lines of it to keep.
///
/// The panel decides how many it can draw; this is only the ceiling on what is
/// carried to it.
const SHOWN_LINES: usize = 8;

/// The last few lines that have anything on them.
///
/// A terminal's screen is mostly blank and mostly rules, and a coding agent
/// draws a good deal of chrome besides. What an operator wants is the last few
/// things that were actually said, so the rules, the empty rows and the status
/// line at the foot are dropped and the words are kept.
fn last_lines(screen: &str, most: usize) -> Vec<String> {
    let mut kept: Vec<String> = screen
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && line.chars().any(char::is_alphanumeric))
        .filter(|line| !is_chrome(line))
        .map(|line| line.chars().take(WIDEST).collect())
        .collect();

    if kept.len() > most {
        kept.drain(..kept.len() - most);
    }
    kept
}

/// How many characters of a line the panel can hold.
///
/// Wider than it looks: the panel is 960 pixels and this type is small, so a
/// line of prose fits whole and only a wrapped paste is cut.
const WIDEST: usize = 150;

/// Whether a line is the terminal's own furniture rather than something said.
fn is_chrome(line: &str) -> bool {
    let trimmed = line.trim();
    // A row of rule characters, which every agent draws between sections.
    let ruled = trimmed
        .chars()
        .all(|c| matches!(c, '─' | '━' | '═' | '-' | '_' | '·'));

    ruled
        || trimmed.contains("shift+tab to cycle")
        || trimmed.starts_with("/clear to save")
        || trimmed.contains("new task? /clear")
}

fn invalid(reason: &'static str) -> ActionError {
    ActionError::backend(
        reason,
        ErrorClass::Validation,
        std::io::Error::other(reason),
    )
}

fn into_action_error(error: AttachError) -> ActionError {
    let class = error.class();
    ActionError::backend(error.to_string(), class, error)
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::ids::CorrelationId;
    use pushos_testkit::{FakeAttached, SessionCall};

    use super::*;

    fn rig() -> (SessionProvider, FakeAttached) {
        let fake = FakeAttached::with_sessions([
            ("/dev/ttys003", "✳ Sprint 2 setup"),
            ("/dev/ttys004", "◐ Review project tasks"),
        ]);
        (SessionProvider::new(Arc::new(fake.clone())), fake)
    }

    fn context(verb: &str, target: Option<&str>) -> ActionContext {
        let mut params = Params::new();
        if let Some(target) = target {
            params.set("target", ParamValue::Text(target.into()));
        }
        ActionContext::new(
            ActionDefinition::new(
                format!("session.{verb}")
                    .parse::<ActionSelector>()
                    .expect("valid"),
                params,
            ),
            CorrelationId::generate(),
            SurfaceContext::empty(),
        )
    }

    fn typing(target: &str, text: &str) -> ActionContext {
        let mut context = context("send", Some(target));
        context
            .definition
            .params
            .set("text", ParamValue::Text(text.into()));
        context
    }

    #[tokio::test]
    async fn a_pad_can_name_a_session_by_part_of_its_title() {
        // Which is what an operator remembers: the sprint work, not ttys003.
        let (provider, _fake) = rig();
        let result = provider
            .execute(context("select", Some("sprint")))
            .await
            .expect("it is open");

        assert_eq!(result.message.as_deref(), Some("Sprint 2 setup"));
        assert_eq!(
            provider.selected().map(|id| id.to_string()),
            Some("/dev/ttys003".to_owned())
        );
    }

    #[tokio::test]
    async fn a_pad_can_name_a_session_by_its_device() {
        let (provider, _fake) = rig();
        provider
            .execute(context("select", Some("tty:ttys004")))
            .await
            .expect("it is open");
        assert_eq!(
            provider.selected().map(|id| id.to_string()),
            Some("/dev/ttys004".to_owned())
        );
    }

    #[tokio::test]
    async fn one_pad_with_no_target_serves_whichever_is_selected() {
        let (provider, fake) = rig();
        provider
            .execute(context("select", Some("review")))
            .await
            .expect("it is open");

        provider
            .execute(typing("selected", "y\n"))
            .await
            .expect("typing succeeds");

        assert!(
            fake.calls().contains(&SessionCall::Sent(
                "/dev/ttys004".to_owned(),
                "y\n".to_owned()
            )),
            "{:?}",
            fake.calls()
        );
    }

    #[tokio::test]
    async fn typing_with_nothing_selected_says_so_rather_than_guessing() {
        // Typing into whichever session happened to be first would be the
        // worst possible answer to an ambiguous instruction.
        let (provider, fake) = rig();
        provider
            .execute(typing("selected", "y\n"))
            .await
            .expect_err("nothing is selected");
        assert!(
            !fake
                .calls()
                .iter()
                .any(|call| matches!(call, SessionCall::Sent(..)))
        );
    }

    #[tokio::test]
    async fn interrupting_sends_the_control_character_not_the_word() {
        let (provider, fake) = rig();
        provider
            .execute(context("interrupt", Some("sprint")))
            .await
            .expect("it is open");

        assert!(
            fake.calls().contains(&SessionCall::Sent(
                "/dev/ttys003".to_owned(),
                "\u{3}".to_owned()
            )),
            "{:?}",
            fake.calls()
        );
    }

    #[tokio::test]
    async fn showing_a_session_puts_the_last_of_it_on_the_display() {
        let (provider, fake) = rig();
        fake.showing(
            "an older line\n\n────────────\nthe tests are running\n❯ \n────────────\nall 40 passed\n",
        );

        let result = provider
            .execute(context("show", Some("sprint")))
            .await
            .expect("it is open");

        let Some(DisplayIntent::Focus { lines, title, .. }) = result.display else {
            panic!("a session should take the whole panel and stay there");
        };
        assert_eq!(title, "Sprint 2 setup");
        assert_eq!(
            lines.last().map(String::as_str),
            Some("all 40 passed"),
            "the last thing said is what matters: {lines:?}"
        );
        assert!(lines.len() <= SHOWN_LINES);
        assert!(
            !lines.iter().any(|line| line.trim() == "────────────"),
            "rules are not what was said: {lines:?}"
        );
    }

    #[tokio::test]
    async fn the_terminals_own_furniture_is_not_shown_as_something_said() {
        // An agent draws a good deal of chrome. Filling the panel with its
        // rules and its status line would leave no room for the answer.
        let (provider, fake) = rig();
        fake.showing(
            "the actual answer\n\
             ────────────────\n\
             ❯ \n\
             ────────────────\n\
             new task? /clear to save 739k tokens\n\
             ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents\n",
        );

        let result = provider
            .execute(context("show", Some("sprint")))
            .await
            .expect("it is open");
        let Some(DisplayIntent::Focus { lines, .. }) = result.display else {
            panic!("focused");
        };
        assert_eq!(lines, ["the actual answer"], "{lines:?}");
    }

    #[tokio::test]
    async fn a_session_that_was_closed_is_reported_rather_than_failing_oddly() {
        let (provider, fake) = rig();
        fake.close("/dev/ttys003");

        let error = provider
            .execute(context("select", Some("sprint")))
            .await
            .expect_err("that window is closed");
        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(error.to_string().contains("sprint"), "{error}");
    }

    #[tokio::test]
    async fn a_pad_can_press_a_key_rather_than_type_a_line() {
        // Accepting a suggestion is Tab and nothing else. Typing a line would
        // accept it and send it, which is a different instruction.
        let (provider, fake) = rig();
        let mut context = context("press", Some("sprint"));
        context
            .definition
            .params
            .set("key", ParamValue::Text("tab".into()));

        provider.execute(context).await.expect("it is open");
        assert!(
            fake.calls()
                .contains(&SessionCall::Pressed("/dev/ttys003".to_owned(), Key::Tab)),
            "{:?}",
            fake.calls()
        );
    }

    #[tokio::test]
    async fn a_key_pushos_cannot_press_is_refused_by_name() {
        let (provider, _fake) = rig();
        let mut context = context("press", Some("sprint"));
        context
            .definition
            .params
            .set("key", ParamValue::Text("f13".into()));

        let error = provider
            .execute(context)
            .await
            .expect_err("there is no such key");
        assert!(error.to_string().contains("f13"), "{error}");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    /// The device a focused view is showing.
    fn showing(result: &ActionResult) -> String {
        match &result.display {
            Some(DisplayIntent::Focus { title, .. }) => title.clone(),
            other => panic!("expected a focused session, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn turning_through_the_sessions_wraps_and_needs_no_first_choice() {
        // An operator reaching for an encoder has not chosen anything yet, and
        // a knob that did nothing until they had would be a knob nobody used.
        let (provider, _fake) = rig();

        assert_eq!(
            showing(&provider.execute(context("next", None)).await.unwrap()),
            "Sprint 2 setup"
        );
        assert_eq!(
            showing(&provider.execute(context("next", None)).await.unwrap()),
            "Review project tasks"
        );
        assert_eq!(
            showing(&provider.execute(context("next", None)).await.unwrap()),
            "Sprint 2 setup",
            "and round again"
        );
    }

    #[tokio::test]
    async fn turning_the_other_way_goes_back() {
        let (provider, _fake) = rig();
        assert_eq!(
            showing(&provider.execute(context("previous", None)).await.unwrap()),
            "Review project tasks",
            "starting from the far end"
        );
        assert_eq!(
            showing(&provider.execute(context("previous", None)).await.unwrap()),
            "Sprint 2 setup"
        );
    }

    #[tokio::test]
    async fn browsing_opens_what_it_arrives_at() {
        // Turning a knob to browse and seeing nothing change would be a knob
        // that does nothing.
        let (provider, _fake) = rig();
        let result = provider.execute(context("next", None)).await.expect("open");
        assert!(matches!(result.display, Some(DisplayIntent::Focus { .. })));
    }

    #[tokio::test]
    async fn scrolling_moves_back_through_what_was_said_and_stops_at_the_end() {
        let (provider, fake) = rig();
        fake.showing("one\ntwo\nthree\nfour\n");
        provider
            .execute(context("select", Some("sprint")))
            .await
            .expect("open");

        let mut back = context("scroll", Some("sprint"));
        back.definition.params.set("by", ParamValue::Integer(2));
        let result = provider.execute(back).await.expect("open");

        let Some(DisplayIntent::Focus { lines, kind, .. }) = result.display else {
            panic!("focused");
        };
        assert_eq!(lines, ["one", "two"], "two lines back");
        assert!(kind.contains("back"), "and it says so: {kind}");
    }

    #[tokio::test]
    async fn scrolling_past_the_beginning_stays_at_the_beginning() {
        let (provider, fake) = rig();
        fake.showing("one\ntwo\n");
        provider
            .execute(context("select", Some("sprint")))
            .await
            .expect("open");

        let mut far = context("scroll", Some("sprint"));
        far.definition.params.set("by", ParamValue::Integer(500));
        provider.execute(far).await.expect("open");

        let mut forward = context("scroll", Some("sprint"));
        forward
            .definition
            .params
            .set("by", ParamValue::Integer(-500));
        let result = provider.execute(forward).await.expect("open");

        let Some(DisplayIntent::Focus { lines, .. }) = result.display else {
            panic!("focused");
        };
        assert_eq!(lines, ["one", "two"], "and back to the bottom");
    }

    #[tokio::test]
    async fn looking_at_something_else_starts_it_at_the_bottom() {
        // A scroll position belongs to the thing it was scrolling.
        let (provider, fake) = rig();
        fake.showing("one\ntwo\nthree\n");
        provider
            .execute(context("select", Some("sprint")))
            .await
            .expect("open");

        let mut back = context("scroll", Some("sprint"));
        back.definition.params.set("by", ParamValue::Integer(2));
        provider.execute(back).await.expect("open");

        let result = provider
            .execute(context("show", Some("review")))
            .await
            .expect("open");
        let Some(DisplayIntent::Focus { lines, .. }) = result.display else {
            panic!("focused");
        };
        assert_eq!(lines.len(), 3, "the new one starts where it is now");
    }

    #[tokio::test]
    async fn only_typing_needs_saying_yes_to() {
        // Looking at a session changes nothing. Typing into one runs whatever
        // it makes of the keystrokes.
        let (provider, _fake) = rig();
        let capabilities = provider.capabilities();

        for typing in ["send", "press", "interrupt"] {
            assert_eq!(
                capabilities.required_for(&ActionVerb::new(typing)),
                [Permission::ShellExecute],
                "`{typing}` types into a live session"
            );
        }
        for looking in ["select", "show", "focus"] {
            assert!(
                capabilities
                    .required_for(&ActionVerb::new(looking))
                    .is_empty(),
                "`{looking}` only looks"
            );
        }
    }

    #[tokio::test]
    async fn a_refused_automation_permission_is_reported_as_one() {
        let (provider, fake) = rig();
        fake.refuse();

        let error = provider
            .execute(context("select", Some("sprint")))
            .await
            .expect_err("permission was refused");
        assert_eq!(error.class(), ErrorClass::Permission);
    }

    #[tokio::test]
    async fn an_unknown_verb_is_refused_by_name() {
        let (provider, _fake) = rig();
        let error = provider
            .execute(context("restart", None))
            .await
            .expect_err("there is no such verb");
        assert!(matches!(error, ActionError::UnknownVerb { .. }));
    }
}
