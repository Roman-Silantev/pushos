//! Every terminal PushOS knows about.
//!
//! One owner, mutated through it, read as summaries. Terminals finish while the
//! surface is being drawn and while actions are running, so nothing outside
//! holds a reference to one.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use pushos_domain::ids::{SessionId, WorkspaceId};
use pushos_domain::ports::{TerminalEvent, TerminalHandle, TerminalSpec};
use pushos_domain::terminal::TerminalTarget;
use tracing::debug;

use crate::session::{TerminalSession, TerminalSummary};

/// How many finished terminals are kept.
///
/// Enough to look back at the last few runs, bounded so that a day of work does
/// not accumulate.
const KEPT_FINISHED: usize = 5;

/// Every terminal, running and recently finished.
#[derive(Debug, Default)]
pub struct TerminalRegistry {
    terminals: HashMap<SessionId, TerminalSession>,
    /// The terminal the operator most recently chose, which unqualified actions
    /// act on.
    selected: Option<SessionId>,
    /// Terminals PushOS was asked to end.
    ///
    /// A killed process and a crashed one look identical from outside, so the
    /// difference has to be remembered rather than inferred.
    stopping: HashSet<SessionId>,
}

impl TerminalRegistry {
    /// Builds an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a terminal that has just opened, and selects it.
    ///
    /// Selecting on open is what makes "start the tests, then watch them" one
    /// gesture rather than two.
    pub fn opened(
        &mut self,
        handle: &TerminalHandle,
        spec: &TerminalSpec,
        workspace: Option<WorkspaceId>,
        now: Instant,
    ) -> TerminalSummary {
        let session = TerminalSession::opened(handle, spec, workspace, now);
        self.selected = Some(session.id.clone());
        let summary = session.summary(true);
        self.terminals.insert(session.id.clone(), session);
        self.forget_stale();
        summary
    }

    /// Applies something a terminal did.
    ///
    /// Returns `None` when the event names a terminal already forgotten, which
    /// is ordinary: output can be in flight when a registry is pruned.
    pub fn apply(
        &mut self,
        terminal: &SessionId,
        event: &TerminalEvent,
        now: Instant,
    ) -> Option<TerminalSummary> {
        let selected = self.selected.as_ref() == Some(terminal);
        let asked_to_stop = self.stopping.remove(terminal);

        let session = self.terminals.get_mut(terminal)?;
        session.apply(event, now);
        if asked_to_stop && matches!(event, TerminalEvent::Exited { .. }) {
            session.stopped(now);
        }
        let summary = session.summary(selected);

        if matches!(event, TerminalEvent::Exited { .. }) {
            self.forget_stale();
        }
        Some(summary)
    }

    /// Records that PushOS has been asked to end a terminal.
    ///
    /// The exit that follows is then reported as a stop rather than a failure.
    pub fn stopping(&mut self, terminal: &SessionId) {
        if self.terminals.contains_key(terminal) {
            self.stopping.insert(terminal.clone());
        }
    }

    /// Marks a terminal as never having got going.
    pub fn failed(&mut self, terminal: &SessionId, now: Instant) {
        if let Some(session) = self.terminals.get_mut(terminal) {
            session.failed(now);
        }
        self.forget_stale();
    }

    /// Works out which terminal a target names.
    ///
    /// A name prefers a running terminal over a finished one, and the most
    /// recently started of several, so "the tests" means the ones in progress.
    pub fn resolve(&self, target: &TerminalTarget) -> Option<SessionId> {
        match target {
            TerminalTarget::Session(id) => self.terminals.contains_key(id).then(|| id.clone()),
            TerminalTarget::Selected => self.selected.clone(),
            TerminalTarget::Named(name) => self
                .terminals
                .values()
                .filter(|session| session.name == *name)
                .max_by_key(|session| (session.is_live(), session.started_at))
                .map(|session| session.id.clone()),
        }
    }

    /// Makes a terminal the one unqualified actions act on.
    ///
    /// Refuses a terminal it does not know, so a stale binding cannot leave the
    /// surface pointing at nothing.
    pub fn select(&mut self, terminal: &SessionId) -> bool {
        if self.terminals.contains_key(terminal) {
            self.selected = Some(terminal.clone());
            true
        } else {
            false
        }
    }

    /// The terminal unqualified actions act on.
    pub fn selected(&self) -> Option<&SessionId> {
        self.selected.as_ref()
    }

    /// Reads one terminal.
    pub fn get(&self, terminal: &SessionId) -> Option<&TerminalSession> {
        self.terminals.get(terminal)
    }

    /// Every terminal, in the order they were opened.
    ///
    /// Opening order rather than status, so a terminal does not move on the
    /// surface at the moment it finishes.
    pub fn summaries(&self) -> Vec<TerminalSummary> {
        let mut sessions: Vec<&TerminalSession> = self.terminals.values().collect();
        sessions.sort_by_key(|session| (session.started_at, session.id.clone()));
        sessions
            .into_iter()
            .map(|session| session.summary(self.selected.as_ref() == Some(&session.id)))
            .collect()
    }

    /// How many terminals are still running.
    pub fn live_count(&self) -> usize {
        self.terminals
            .values()
            .filter(|session| session.is_live())
            .count()
    }

    /// Removes a terminal outright.
    pub fn forget(&mut self, terminal: &SessionId) {
        self.terminals.remove(terminal);
        self.stopping.remove(terminal);
        if self.selected.as_ref() == Some(terminal) {
            // Fall back to whatever is still running rather than leaving the
            // surface pointing at nothing.
            self.selected = self
                .terminals
                .values()
                .filter(|session| session.is_live())
                .max_by_key(|session| session.last_activity)
                .map(|session| session.id.clone());
        }
    }

    /// Drops all but the most recent finished terminals.
    fn forget_stale(&mut self) {
        let mut finished: Vec<(SessionId, Instant)> = self
            .terminals
            .values()
            .filter(|session| !session.is_live())
            .map(|session| (session.id.clone(), session.last_activity))
            .collect();

        if finished.len() <= KEPT_FINISHED {
            return;
        }

        finished.sort_by_key(|(id, at)| (Reverse(*at), id.clone()));
        for (id, _) in finished.split_off(KEPT_FINISHED) {
            debug!(%id, "forgetting a finished terminal");
            self.forget(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::ports::TerminalStatus;

    use super::*;

    struct Fixture {
        registry: TerminalRegistry,
        now: Instant,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                registry: TerminalRegistry::new(),
                now: Instant::now(),
            }
        }

        fn tick(&mut self) -> Instant {
            self.now += std::time::Duration::from_millis(1);
            self.now
        }

        fn open(&mut self, id: &str, name: &str) -> SessionId {
            let handle = TerminalHandle {
                id: SessionId::new(id),
                pid: None,
            };
            let spec = TerminalSpec::new(name, "bash", "/tmp");
            let at = self.tick();
            self.registry.opened(&handle, &spec, None, at);
            handle.id
        }

        fn exit(&mut self, id: &SessionId, code: i32) {
            let at = self.tick();
            self.registry
                .apply(id, &TerminalEvent::Exited { code: Some(code) }, at);
        }
    }

    #[test]
    fn opening_a_terminal_selects_it() {
        let mut fixture = Fixture::new();
        let id = fixture.open("t1", "tests");
        assert_eq!(fixture.registry.selected(), Some(&id));
    }

    #[test]
    fn a_name_finds_the_running_terminal_rather_than_the_finished_one() {
        let mut fixture = Fixture::new();
        let old = fixture.open("t1", "tests");
        fixture.exit(&old, 0);
        let new = fixture.open("t2", "tests");

        assert_eq!(
            fixture
                .registry
                .resolve(&TerminalTarget::Named("tests".to_owned())),
            Some(new)
        );
    }

    #[test]
    fn a_name_still_finds_a_finished_terminal_when_none_is_running() {
        // "How did the tests go" has to work after they finish.
        let mut fixture = Fixture::new();
        let id = fixture.open("t1", "tests");
        fixture.exit(&id, 1);

        assert_eq!(
            fixture
                .registry
                .resolve(&TerminalTarget::Named("tests".to_owned())),
            Some(id)
        );
    }

    #[test]
    fn a_name_nothing_holds_resolves_to_nothing() {
        let mut fixture = Fixture::new();
        fixture.open("t1", "tests");
        assert_eq!(
            fixture
                .registry
                .resolve(&TerminalTarget::Named("build".to_owned())),
            None
        );
    }

    #[test]
    fn an_exact_identifier_only_matches_a_terminal_that_exists() {
        let mut fixture = Fixture::new();
        let id = fixture.open("t1", "tests");

        assert_eq!(
            fixture
                .registry
                .resolve(&TerminalTarget::Session(id.clone())),
            Some(id)
        );
        assert_eq!(
            fixture
                .registry
                .resolve(&TerminalTarget::Session(SessionId::new("gone"))),
            None
        );
    }

    #[test]
    fn selecting_a_terminal_that_is_not_there_changes_nothing() {
        let mut fixture = Fixture::new();
        let id = fixture.open("t1", "tests");

        assert!(!fixture.registry.select(&SessionId::new("gone")));
        assert_eq!(fixture.registry.selected(), Some(&id));
    }

    #[test]
    fn forgetting_the_selected_terminal_falls_back_to_one_still_running() {
        let mut fixture = Fixture::new();
        let first = fixture.open("t1", "build");
        let second = fixture.open("t2", "tests");
        assert_eq!(fixture.registry.selected(), Some(&second));

        fixture.registry.forget(&second);
        assert_eq!(fixture.registry.selected(), Some(&first));
    }

    #[test]
    fn forgetting_the_last_terminal_leaves_nothing_selected() {
        let mut fixture = Fixture::new();
        let id = fixture.open("t1", "tests");
        fixture.registry.forget(&id);
        assert_eq!(fixture.registry.selected(), None);
    }

    #[test]
    fn finished_terminals_do_not_accumulate() {
        let mut fixture = Fixture::new();
        for index in 0..KEPT_FINISHED * 4 {
            let id = fixture.open(&format!("t{index}"), "run");
            fixture.exit(&id, 0);
        }
        assert_eq!(fixture.registry.summaries().len(), KEPT_FINISHED);
    }

    #[test]
    fn pruning_never_drops_a_running_terminal() {
        let mut fixture = Fixture::new();
        let running = fixture.open("live", "server");
        for index in 0..KEPT_FINISHED * 4 {
            let id = fixture.open(&format!("t{index}"), "run");
            fixture.exit(&id, 0);
        }

        assert_eq!(fixture.registry.live_count(), 1);
        assert!(fixture.registry.get(&running).is_some());
    }

    #[test]
    fn terminals_are_listed_in_the_order_they_were_opened() {
        let mut fixture = Fixture::new();
        fixture.open("t1", "first");
        fixture.open("t2", "second");
        fixture.open("t3", "third");

        let names: Vec<String> = fixture
            .registry
            .summaries()
            .into_iter()
            .map(|summary| summary.name)
            .collect();
        assert_eq!(names, ["first", "second", "third"]);
    }

    #[test]
    fn a_terminal_that_finishes_does_not_move_in_the_list() {
        // A pad that meant one terminal must not come to mean another because
        // something above it exited.
        let mut fixture = Fixture::new();
        let first = fixture.open("t1", "first");
        fixture.open("t2", "second");
        fixture.exit(&first, 0);

        let names: Vec<String> = fixture
            .registry
            .summaries()
            .into_iter()
            .map(|summary| summary.name)
            .collect();
        assert_eq!(names, ["first", "second"]);
    }

    #[test]
    fn output_for_a_forgotten_terminal_is_ignored_rather_than_a_fault() {
        let mut fixture = Fixture::new();
        let at = fixture.tick();
        let late = fixture.registry.apply(
            &SessionId::new("gone"),
            &TerminalEvent::Output {
                text: "hello".to_owned(),
            },
            at,
        );
        assert!(late.is_none());
    }

    #[test]
    fn a_terminal_pushos_was_asked_to_end_is_reported_as_stopped() {
        // A killed process and a crashed one look the same from outside, so
        // the surface would show red for a deliberate stop.
        let mut fixture = Fixture::new();
        let id = fixture.open("t1", "server");
        fixture.registry.stopping(&id);

        let at = fixture.tick();
        fixture
            .registry
            .apply(&id, &TerminalEvent::Exited { code: None }, at);

        let session = fixture.registry.get(&id).expect("it is still known");
        assert_eq!(session.status, TerminalStatus::Stopped);
        assert!(!session.status.is_failure());
    }

    #[test]
    fn a_terminal_that_ends_on_its_own_is_not_reported_as_stopped() {
        let mut fixture = Fixture::new();
        let id = fixture.open("t1", "tests");
        fixture.exit(&id, 1);

        let session = fixture.registry.get(&id).expect("it is still known");
        assert_eq!(session.status, TerminalStatus::Exited { code: Some(1) });
        assert!(session.status.is_failure());
    }

    #[test]
    fn a_terminal_that_failed_to_start_is_reported_as_failed() {
        let mut fixture = Fixture::new();
        let id = fixture.open("t1", "tests");
        let at = fixture.tick();
        fixture.registry.failed(&id, at);

        let session = fixture.registry.get(&id).expect("it is still known");
        assert_eq!(session.status, TerminalStatus::Failed);
    }
}
