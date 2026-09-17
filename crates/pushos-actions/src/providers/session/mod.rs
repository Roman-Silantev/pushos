//! Driving sessions the operator already had open, or asked for by name.
//!
//! PushOS did not start these and does not own them, so this namespace is
//! deliberately narrower than the one for terminals PushOS runs. It can say
//! which one it is looking at, bring one to the front, type into one, and stop
//! one. It cannot tell you what any of them exited with, because it will never
//! find out.
//!
//! It can also ask for one by name, for a pad that keeps a worker: the first
//! press starts the session somewhere that keeps it running by itself, and
//! every press after that comes back to it.

mod lines;
mod questions;
mod turns;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionResult, ActionStatus, Depth, DisplayIntent};
use pushos_domain::attached::{
    Activity, Attached, AttachedTarget, BANK, Decision, SessionQuestion, is_session_name,
};
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::{ActionVerb, AttachedId, ProviderName};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::{
    ActionProvider, AttachError, AttachedSessions, Keeper, Key, OpenSession, ProviderCapabilities,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tracing::{debug, info, warn};

use self::questions::Questions;

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
    /// Which bank of eight the surface is showing.
    ///
    /// Published, because the display draws that bank and the lights under the
    /// pads have to agree with it. Eight pads serve any number of sessions by
    /// moving this rather than by meaning something different.
    bank: watch::Sender<usize>,
    /// How far back through what a session said the operator has scrolled.
    ///
    /// Lines from the bottom. Reset whenever they look at something else,
    /// because a scroll position belongs to the thing it was scrolling.
    scrolled: AtomicUsize,
    /// What was open, and what one of them had said, as last seen.
    ///
    /// Every answer here costs a process, and a knob being turned asks many
    /// times a second. Believing the last answer for a fraction of one keeps
    /// the surface honest without spawning `osascript` for every detent.
    seen: Mutex<Seen>,
    /// Questions sessions are waiting on the operator to answer from here.
    questions: Questions,
    /// How many sessions may work at once, and what is waiting its turn.
    turns: turns::Turns,
    /// Where the projects are, when PushOS knows about projects.
    ///
    /// What makes one row of role pads serve twenty projects: a pad that says
    /// which role it is, and no folder, works in whichever project is in
    /// effect.
    workspaces: Option<Arc<dyn pushos_domain::ports::WorkspaceContext>>,
}

/// The last answers, and when they were given.
#[derive(Debug, Default)]
struct Seen {
    open: Option<(Vec<Attached>, Instant)>,
    said: Option<(AttachedId, Vec<String>, Instant)>,
}

/// How long one of those answers is believed for.
///
/// Short enough that nothing on the panel is visibly behind what a session is
/// doing, and long enough that a knob spun through a long history is a handful
/// of looks rather than one for every detent.
const BELIEVED: Duration = Duration::from_millis(300);

impl Seen {
    /// Whether an answer given at `at` is still worth believing.
    fn fresh(at: Instant, now: Instant) -> bool {
        now.duration_since(at) < BELIEVED
    }
}

impl SessionProvider {
    /// Builds the provider over whatever can see other terminals.
    pub fn new(sessions: Arc<dyn AttachedSessions>) -> Self {
        Self {
            sessions,
            selected: watch::channel(None).0,
            bank: watch::channel(0).0,
            scrolled: AtomicUsize::new(0),
            seen: Mutex::new(Seen::default()),
            questions: Questions::default(),
            turns: turns::Turns::new(),
            workspaces: None,
        }
    }

    /// Lets a pad with no folder of its own work in the project in effect.
    #[must_use]
    pub fn following_projects(
        mut self,
        workspaces: Arc<dyn pushos_domain::ports::WorkspaceContext>,
    ) -> Self {
        self.workspaces = Some(workspaces);
        self
    }

    /// Puts a session's question to the operator, and waits for the answer.
    ///
    /// `None` when no answer came from here: nobody pressed anything in time,
    /// or it was answered in the session's own window first.
    pub async fn ask(&self, question: SessionQuestion) -> Option<Decision> {
        info!(question = %question.describe(), "a session is asking the operator");
        self.questions.ask(question, questions::PATIENCE).await
    }

    /// Follows the questions waiting on the operator, for the display.
    pub fn questions(&self) -> watch::Receiver<Vec<SessionQuestion>> {
        self.questions.watch()
    }

    /// Withdraws the questions the open sessions have stopped asking.
    pub fn settle(&self, open: &[Attached]) {
        self.questions.settle(open);
    }

    /// Answers a question: the one the target's session is asking, or with no
    /// target, the oldest one waiting, so a single pair of buttons answers
    /// every session.
    async fn decide(
        &self,
        context: &ActionContext,
        decision: Decision,
    ) -> Result<ActionResult, ActionError> {
        let answered = if context.params().text("target").is_some() {
            let session = self.resolve(context).await?;
            self.questions
                .answer(|question| question.is_from(&session), decision)
                .ok_or_else(|| invalid("that session is not asking anything"))?
        } else {
            self.questions
                .answer(|_| true, decision)
                .ok_or_else(|| invalid("nothing is asking"))?
        };

        let said = match decision {
            Decision::Allow => "allowed",
            Decision::Deny => "denied",
        };
        info!(question = %answered.describe(), said, "answered from the Push");
        Ok(ActionResult {
            status: ActionStatus::Completed,
            message: Some(format!("{said}: {}", answered.describe())),
            display: Some(DisplayIntent::Toast {
                title: format!("{} {said}", answered.tool),
                detail: Some(answered.agent),
            }),
        })
    }

    /// Follows which session the operator is looking at.
    pub fn watch(&self) -> watch::Receiver<Option<AttachedId>> {
        self.selected.subscribe()
    }

    /// Follows which bank of eight the surface is showing.
    pub fn banked(&self) -> watch::Receiver<usize> {
        self.bank.subscribe()
    }

    /// Shows one bank of eight outright, counted from one.
    ///
    /// Clamped rather than refused: eight buttons stand for eight banks
    /// whether or not there are enough sessions to fill them, and a button
    /// that did nothing on a quiet day would be one nobody trusts.
    fn show_bank(&self, bank: i64, open: usize) -> usize {
        let ordinal = usize::try_from(bank.max(1)).unwrap_or(1);
        let start = ordinal.saturating_sub(1).saturating_mul(BANK);
        let last = open.saturating_sub(1) / BANK * BANK;

        let moved = start.min(last);
        self.bank.send_replace(moved);
        moved
    }

    /// Where the bank starts, counted in sessions.
    pub fn showing_from(&self) -> usize {
        *self.bank.borrow()
    }

    /// Moves the bank, and reports where it left it.
    ///
    /// Clamped rather than wrapped: an operator paging through a list wants to
    /// arrive at the end of it, not back at the beginning without noticing.
    fn move_bank(&self, by: i64, open: usize) -> usize {
        let last = open.saturating_sub(1) / BANK * BANK;
        let held = self.showing_from();

        let moved = if by >= 0 {
            held.saturating_add(usize::try_from(by).unwrap_or(0).saturating_mul(BANK))
        } else {
            held.saturating_sub(
                usize::try_from(by.unsigned_abs())
                    .unwrap_or(0)
                    .saturating_mul(BANK),
            )
        };

        let moved = moved.min(last);
        self.bank.send_replace(moved);
        moved
    }

    /// The one the operator is looking at, if any.
    pub fn selected(&self) -> Option<AttachedId> {
        self.selected.borrow().clone()
    }

    /// What can be seen right now.
    pub async fn discover(&self) -> Result<Vec<Attached>, AttachError> {
        self.sessions.discover().await
    }

    /// What is open, as last seen, asking again only when that has gone stale.
    async fn open(&self) -> Result<Vec<Attached>, ActionError> {
        let now = Instant::now();
        if let Ok(seen) = self.seen.lock()
            && let Some((open, at)) = &seen.open
            && Seen::fresh(*at, now)
        {
            return Ok(open.clone());
        }

        let open = self.sessions.discover().await.map_err(into_action_error)?;
        if let Ok(mut seen) = self.seen.lock() {
            seen.open = Some((open.clone(), Instant::now()));
        }
        Ok(open)
    }

    /// Forgets what was open, so the next question is asked afresh.
    fn forget(&self) {
        if let Ok(mut seen) = self.seen.lock() {
            seen.open = None;
        }
    }

    /// Starts the session a pad names, or comes back to it, and looks at it.
    ///
    /// A window is opened onto it unless the binding says not to, because a
    /// worker that started where nobody can see it would look like a pad that
    /// did nothing.
    async fn open_named(&self, context: &ActionContext) -> Result<ActionResult, ActionError> {
        let params = context.params();
        let name = params.require_text("name")?.trim();
        if !is_session_name(name) {
            return Err(ActionError::backend(
                format!(
                    "`{name}` cannot name a session; use letters, digits, hyphens and underscores"
                ),
                ErrorClass::Validation,
                std::io::Error::other("unusable session name"),
            ));
        }
        // A session in a terminal is a program running for as long as it is
        // open. One a coding agent keeps costs nothing while it is not
        // working, which is what lets every pad have one.
        let keeper: Keeper = params
            .text("keeper")
            .unwrap_or("terminal")
            .parse()
            .map_err(|unknown: pushos_domain::ports::UnknownKeeper| {
                ActionError::backend(
                    unknown.to_string(),
                    ErrorClass::Validation,
                    std::io::Error::other("unknown keeper"),
                )
            })?;
        // A pad with no folder of its own is a role rather than a worker: it
        // works in whichever project is in effect, and its session is that
        // project's. One row of eight roles then serves every project the
        // operator has, instead of eight pads each.
        let (name, directory) = match params.text("cwd") {
            Some(given) => (name.to_owned(), Some(PathBuf::from(given))),
            None => self.in_the_project(name, context).await,
        };
        let request = OpenSession {
            name,
            directory,
            keeper,
            // What to run, for a session in a terminal; what to work on, for
            // one a coding agent keeps.
            command: params
                .text("command")
                .or_else(|| params.text("work"))
                .map(ToOwned::to_owned),
        };

        let opened = self
            .sessions
            .open(&request)
            .await
            .map_err(into_action_error)?;
        self.forget();
        let session = self
            .open()
            .await?
            .into_iter()
            .find(|session| session.id == opened.id)
            .ok_or_else(|| invalid("the session started but could not be seen"))?;
        self.select(&session);

        let window = params
            .get("window")
            .and_then(pushos_domain::action::ParamValue::as_flag)
            .unwrap_or(true);
        if window {
            self.sessions
                .focus(&session.id)
                .await
                .map_err(into_action_error)?;
        }

        let said = if opened.started { "started" } else { "open" };
        Ok(ActionResult {
            status: ActionStatus::Completed,
            message: Some(format!("{} {said}", session.label())),
            display: Some(DisplayIntent::Toast {
                title: session.label().to_owned(),
                detail: Some(said.to_owned()),
            }),
        })
    }

    /// A role's name and folder within the project in effect.
    ///
    /// The name carries the project, so `builder` in two projects is two
    /// sessions rather than one that keeps changing folder.
    async fn in_the_project(
        &self,
        role: &str,
        context: &ActionContext,
    ) -> (String, Option<PathBuf>) {
        let Some(workspaces) = &self.workspaces else {
            return (role.to_owned(), None);
        };
        let workspace = context.surface.workspace.clone();
        let root = workspaces.root(workspace.as_ref()).await;
        let name = workspace
            .map(|project| format!("{project}-{role}"))
            // A project whose name is not one a session can be kept under
            // leaves the role to stand alone rather than making a name
            // nothing can hold.
            .filter(|name| is_session_name(name))
            .unwrap_or_else(|| role.to_owned());
        (name, Some(root))
    }

    /// Whether work for this session would have to wait for a turn.
    ///
    /// Only for a session an agent keeps: one in a terminal is a program the
    /// operator is looking at, and typing into it is theirs to time.
    async fn would_wait(&self, session: &Attached) -> bool {
        if !session.can_be_put_away() || self.turns.limit().is_none() {
            return false;
        }
        let working = self
            .open()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|open| open.can_be_put_away() && open.activity == Activity::Working)
            .count();
        self.turns.busy(working)
    }

    /// Puts work aside until a session frees up, and says where it stands.
    fn wait_for_a_turn(&self, session: &Attached, text: &str) -> Result<ActionResult, ActionError> {
        let ahead = self.turns.hold(&session.id, text).map_err(|full| {
            ActionError::backend(
                full.to_string(),
                ErrorClass::UserActionRequired,
                std::io::Error::other("the queue is full"),
            )
        })?;
        Ok(ActionResult {
            status: ActionStatus::Started,
            message: Some(format!(
                "{} waits its turn ({ahead} to go)",
                session.label()
            )),
            display: Some(DisplayIntent::Toast {
                title: session.label().to_owned(),
                detail: Some(format!("{ahead} waiting")),
            }),
        })
    }

    /// Says how many sessions may work at once, as the configuration has it.
    pub fn allow_working(&self, most: Option<usize>) {
        self.turns.allow(most.unwrap_or(0));
    }

    /// Whether this session has work waiting for a free turn.
    pub fn work_waits_for(&self, session: &AttachedId) -> bool {
        self.turns.holds_work_for(session)
    }

    /// How much work is waiting for a turn.
    pub fn waiting_turns(&self) -> usize {
        self.turns.waiting()
    }

    /// Drops work waiting for sessions that are no longer open.
    pub fn keep_work_for(&self, open: &[Attached]) {
        let there: Vec<AttachedId> = open.iter().map(|session| session.id.clone()).collect();
        self.turns.keep_only(&there);
    }

    /// Sends what has been waiting, up to `free` pieces of work.
    ///
    /// Returns how many went. Work for a session that has since gone is
    /// dropped rather than retried for ever.
    pub async fn start_waiting_work(&self, free: usize) -> usize {
        let mut started = 0;
        for held in self.turns.take(free, Instant::now()) {
            let session = held.session.clone();
            match self.sessions.send(&session, &held.text).await {
                Ok(()) => {
                    debug!(session = %session, "started work that was waiting its turn");
                    started += 1;
                }
                // A session that has gone cannot be told anything, ever.
                Err(AttachError::Gone { .. }) => {
                    debug!(session = %session, "dropped work for a session that has gone");
                }
                // Anything else is usually the model saying there is too much
                // at once, which passes: the work waits rather than being lost.
                Err(error) => {
                    if self.turns.refused(held, Instant::now()) {
                        debug!(%error, session = %session, "work was refused; it waits and goes again");
                    } else {
                        warn!(%error, session = %session, "gave up on work the agent kept refusing");
                    }
                }
            }
        }
        started
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

        let open = self.open().await?;

        if let AttachedTarget::Slot(at) = target {
            // Counted within the bank in view, so a pad means "the third of
            // these" and keeps that meaning as the bank moves.
            let index = self.showing_from() + usize::from(at) - 1;
            return open
                .into_iter()
                .nth(index)
                .ok_or_else(|| invalid("nothing is open in that position"));
        }

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
        let open = self.open().await?;
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

    /// Everything a session has said that is worth showing, oldest first.
    ///
    /// Believed briefly, for the same reason the list of open sessions is:
    /// spinning a knob through what one said is many questions about a thing
    /// that has not changed.
    async fn history(&self, session: &Attached) -> Result<Vec<String>, ActionError> {
        let now = Instant::now();
        if let Ok(seen) = self.seen.lock()
            && let Some((id, said, at)) = &seen.said
            && *id == session.id
            && Seen::fresh(*at, now)
        {
            return Ok(said.clone());
        }

        let screen = self
            .sessions
            .read(&session.id, READ_MOST)
            .await
            .map_err(into_action_error)?;
        let said = lines::last_lines(&screen, READ_MOST);
        if let Ok(mut seen) = self.seen.lock() {
            seen.said = Some((session.id.clone(), said.clone(), Instant::now()));
        }
        Ok(said)
    }

    /// Builds the focused view of a session, at wherever it is scrolled to.
    ///
    /// The panel holds a window rather than the whole history, so this is where
    /// the two meet: the operator's scroll position decides which window, and
    /// how deep that window is goes with it so the panel can draw it.
    fn view(&self, session: &Attached, said: &[String]) -> ActionResult {
        let deepest = Depth::deepest(said.len(), SHOWN_LINES);
        let back = self.scrolled.load(Ordering::Relaxed).min(deepest);
        let end = said.len() - back;
        let start = end.saturating_sub(SHOWN_LINES);

        ActionResult {
            status: ActionStatus::Completed,
            message: Some(session.label().to_owned()),
            // Focused rather than reported: an operator who tapped a pad to
            // read what a session is doing is reading, and a panel that
            // reverted under them after six seconds would be one they could
            // not use.
            display: Some(DisplayIntent::Focus {
                kind: if back == 0 {
                    session.detail().to_owned()
                } else {
                    format!("{} +{back} back", session.detail())
                },
                title: session.label().to_owned(),
                state: session.activity.describe().to_owned(),
                tone: session.activity.status_color(),
                lines: said[start..end].to_vec(),
                depth: Depth::of(said.len(), SHOWN_LINES, back),
            }),
        }
    }

    /// Reads a session and builds the view of it in one go.
    async fn focused(&self, session: &Attached) -> Result<ActionResult, ActionError> {
        let said = self.history(session).await?;
        Ok(self.view(session, &said))
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
                "bank",
                "show",
                "scroll",
                "focus",
                "send",
                "press",
                "interrupt",
                "open",
                "put_away",
                "approve",
                "deny",
            ]
            .map(ActionVerb::new),
        )
        // Typing into a session runs whatever it makes of the keystrokes, which
        // is the same thing a terminal action does and needs the same saying
        // yes to. Looking at one needs nothing.
        .verb_requiring(ActionVerb::new("send"), [Permission::ShellExecute])
        .verb_requiring(ActionVerb::new("press"), [Permission::ShellExecute])
        .verb_requiring(ActionVerb::new("interrupt"), [Permission::ShellExecute])
        // Starting a session types its command into a shell.
        .verb_requiring(ActionVerb::new("open"), [Permission::ShellExecute])
        // Putting one away ends the program running it, which is as much of a
        // decision as starting it was.
        .verb_requiring(ActionVerb::new("put_away"), [Permission::ShellExecute])
        // Allowing lets a session run what it asked to. Denying stops
        // something, and needs nothing.
        .verb_requiring(ActionVerb::new("approve"), [Permission::ShellExecute])
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
                        detail: Some(session.detail().to_owned()),
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

            // Moving the eight the surface is showing, so any number of
            // sessions is reachable from eight pads.
            "bank" => {
                let open = self.open().await?;

                // `to` is a bank counted from one, for a control that jumps
                // straight to it. `by` is a step, for one that walks. A control
                // that named both would be two controls.
                let named = context
                    .params()
                    .get("to")
                    .and_then(pushos_domain::action::ParamValue::as_integer);

                let from = if let Some(bank) = named {
                    self.show_bank(bank, open.len())
                } else {
                    let by = context
                        .params()
                        .get("by")
                        .and_then(pushos_domain::action::ParamValue::as_integer)
                        .unwrap_or(1);
                    self.move_bank(by, open.len())
                };
                let last = (from + BANK).min(open.len());

                Ok(ActionResult {
                    status: ActionStatus::Completed,
                    message: Some(format!("{}-{last} of {}", from + 1, open.len())),
                    display: Some(DisplayIntent::Toast {
                        title: format!("sessions {}-{last}", from + 1),
                        detail: Some(format!("of {}", open.len())),
                    }),
                })
            }

            "scroll" => {
                let by = context
                    .params()
                    .get("by")
                    .and_then(pushos_domain::action::ParamValue::as_integer)
                    .unwrap_or(-1);

                let session = self.resolve(&context).await?;
                // Reading something is looking at it. Without this a knob
                // aimed at one session would move a position belonging to
                // another, because a scroll position belongs to the thing it
                // was scrolling and there is one of them. Selecting the one
                // already selected changes nothing.
                self.select(&session);
                let said = self.history(&session).await?;
                self.scroll(by, Depth::deepest(said.len(), SHOWN_LINES));
                Ok(self.view(&session, &said))
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
                let interrupting = context.definition.selector.verb.as_str() == "interrupt";
                let text = if interrupting {
                    INTERRUPT.to_owned()
                } else {
                    let written = context.params().require_text("text")?;
                    if written.is_empty() {
                        return Err(invalid("there is nothing to type"));
                    }
                    written.to_owned()
                };

                // Stopping something is never made to wait: it is what an
                // operator does when the fleet is already too busy.
                if !interrupting && self.would_wait(&session).await {
                    return self.wait_for_a_turn(&session, &text);
                }

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

            "open" => self.open_named(&context).await,

            "put_away" => {
                let session = self.resolve(&context).await?;
                let away = self
                    .sessions
                    .put_away(&session.id)
                    .await
                    .map_err(into_action_error)?;
                self.forget();
                Ok(ActionResult {
                    status: ActionStatus::Completed,
                    message: Some(if away {
                        format!("{} put away", session.label())
                    } else {
                        format!("{} runs in a terminal; it stays", session.label())
                    }),
                    display: None,
                })
            }

            "approve" => self.decide(&context, Decision::Allow).await,
            "deny" => self.decide(&context, Decision::Deny).await,

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

/// How many lines of it the panel shows at once.
///
/// The window an operator reads. Everything behind it is still there and is
/// what the scroll knobs move through.
const SHOWN_LINES: usize = 8;

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
mod tests;
