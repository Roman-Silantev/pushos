//! A terminal host that starts nothing.
//!
//! Satisfies the same port as the real pseudo-terminal adapter, so the
//! supervisor, the registry and every action above them can be exercised
//! without spawning a process or waiting on a kernel.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use pushos_domain::error::ErrorClass;
use pushos_domain::ids::SessionId;
use pushos_domain::ports::{
    TerminalError, TerminalEvent, TerminalHandle, TerminalHost, TerminalObserver, TerminalSize,
    TerminalSpec,
};

/// One terminal a fake host is pretending to run.
#[derive(Clone, Debug)]
struct FakeOpen {
    spec: TerminalSpec,
    typed: String,
    size: TerminalSize,
}

/// A terminal host that records instead of running.
#[derive(Debug, Clone)]
pub struct FakeTerminal {
    observer: Arc<dyn TerminalObserver>,
    open: Arc<Mutex<HashMap<SessionId, FakeOpen>>>,
    fail_with: Arc<Mutex<Option<ErrorClass>>>,
    /// Ended terminals are kept so a test can still read what was typed.
    closed: Arc<Mutex<Vec<SessionId>>>,
}

impl FakeTerminal {
    /// Builds a host reporting to `observer`.
    pub fn new(observer: Arc<dyn TerminalObserver>) -> Self {
        Self {
            observer,
            open: Arc::new(Mutex::new(HashMap::new())),
            fail_with: Arc::new(Mutex::new(None)),
            closed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Makes every subsequent call fail with the given classification.
    pub fn fail_with(&self, class: ErrorClass) {
        *lock(&self.fail_with) = Some(class);
    }

    /// Stops making calls fail.
    pub fn recover(&self) {
        *lock(&self.fail_with) = None;
    }

    /// What was typed into a terminal, in order.
    pub fn typed(&self, terminal: &SessionId) -> Option<String> {
        lock(&self.open)
            .get(terminal)
            .map(|open| open.typed.clone())
    }

    /// What a terminal was asked to run.
    pub fn spec(&self, terminal: &SessionId) -> Option<TerminalSpec> {
        lock(&self.open).get(terminal).map(|open| open.spec.clone())
    }

    /// The size a terminal was last told it is.
    pub fn size(&self, terminal: &SessionId) -> Option<TerminalSize> {
        lock(&self.open).get(terminal).map(|open| open.size)
    }

    /// Every terminal that was asked to stop, in order.
    pub fn closed(&self) -> Vec<SessionId> {
        lock(&self.closed).clone()
    }

    /// How many terminals are running.
    pub fn open_count(&self) -> usize {
        lock(&self.open).len()
    }

    /// Pretends a terminal produced output.
    pub fn emit(&self, terminal: &SessionId, text: &str) {
        self.observer.observe(
            terminal,
            TerminalEvent::Output {
                text: text.to_owned(),
            },
        );
    }

    /// Pretends a terminal finished, the way the real host would report it.
    pub fn finish(&self, terminal: &SessionId, code: Option<i32>) {
        lock(&self.open).remove(terminal);
        self.observer
            .observe(terminal, TerminalEvent::Exited { code });
    }

    fn check(&self) -> Result<(), TerminalError> {
        match *lock(&self.fail_with) {
            None => Ok(()),
            Some(class) => Err(TerminalError::host(
                "the fake was told to fail",
                class,
                std::io::Error::other("injected failure"),
            )),
        }
    }
}

#[async_trait]
impl TerminalHost for FakeTerminal {
    async fn open(
        &self,
        id: SessionId,
        spec: TerminalSpec,
    ) -> Result<TerminalHandle, TerminalError> {
        self.check()?;
        let size = spec.size;
        lock(&self.open).insert(
            id.clone(),
            FakeOpen {
                spec,
                typed: String::new(),
                size,
            },
        );
        Ok(TerminalHandle {
            id,
            pid: Some(4242),
        })
    }

    async fn write(&self, terminal: &SessionId, input: &str) -> Result<(), TerminalError> {
        self.check()?;
        let mut open = lock(&self.open);
        let entry = open
            .get_mut(terminal)
            .ok_or_else(|| TerminalError::NoSuchTerminal {
                terminal: terminal.clone(),
            })?;
        entry.typed.push_str(input);
        Ok(())
    }

    async fn resize(&self, terminal: &SessionId, size: TerminalSize) -> Result<(), TerminalError> {
        self.check()?;
        let mut open = lock(&self.open);
        let entry = open
            .get_mut(terminal)
            .ok_or_else(|| TerminalError::NoSuchTerminal {
                terminal: terminal.clone(),
            })?;
        entry.size = size;
        Ok(())
    }

    async fn close(&self, terminal: &SessionId) -> Result<(), TerminalError> {
        self.check()?;
        if !lock(&self.open).contains_key(terminal) {
            return Err(TerminalError::NoSuchTerminal {
                terminal: terminal.clone(),
            });
        }
        lock(&self.closed).push(terminal.clone());

        // The real host reports the exit through the observer rather than
        // returning it, and so does this one.
        self.finish(terminal, None);
        Ok(())
    }
}

/// Records everything a host reports, for a test to read back.
#[derive(Debug, Clone, Default)]
pub struct RecordingTerminalObserver {
    events: Arc<Mutex<Vec<(SessionId, TerminalEvent)>>>,
}

impl RecordingTerminalObserver {
    /// Builds an observer that has seen nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything reported, in order.
    pub fn events(&self) -> Vec<(SessionId, TerminalEvent)> {
        lock(&self.events).clone()
    }

    /// Everything one terminal produced, joined.
    pub fn output(&self, terminal: &SessionId) -> String {
        lock(&self.events)
            .iter()
            .filter(|(id, _)| id == terminal)
            .filter_map(|(_, event)| match event {
                TerminalEvent::Output { text } => Some(text.as_str()),
                TerminalEvent::Exited { .. } => None,
            })
            .collect()
    }

    /// How a terminal ended, if it has.
    pub fn exit_of(&self, terminal: &SessionId) -> Option<Option<i32>> {
        lock(&self.events)
            .iter()
            .find_map(|(id, event)| match event {
                TerminalEvent::Exited { code } if id == terminal => Some(*code),
                _ => None,
            })
    }
}

impl TerminalObserver for RecordingTerminalObserver {
    fn observe(&self, terminal: &SessionId, event: TerminalEvent) {
        lock(&self.events).push((terminal.clone(), event));
    }
}

/// Takes a lock without turning a panic elsewhere into a panic here.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> (FakeTerminal, RecordingTerminalObserver) {
        let observer = RecordingTerminalObserver::new();
        (FakeTerminal::new(Arc::new(observer.clone())), observer)
    }

    #[tokio::test]
    async fn what_is_typed_is_recorded_in_order() {
        let (terminals, _) = host();
        let id = SessionId::new("t1");
        terminals
            .open(id.clone(), TerminalSpec::new("tests", "bash", "/tmp"))
            .await
            .expect("the fake succeeds by default");

        terminals.write(&id, "cargo ").await.expect("it is open");
        terminals.write(&id, "test\r").await.expect("it is open");
        assert_eq!(terminals.typed(&id).as_deref(), Some("cargo test\r"));
    }

    #[tokio::test]
    async fn typing_into_a_terminal_that_is_not_there_is_refused() {
        let (terminals, _) = host();
        let error = terminals
            .write(&SessionId::new("gone"), "x")
            .await
            .expect_err("nothing is open");
        assert!(matches!(error, TerminalError::NoSuchTerminal { .. }));
    }

    #[tokio::test]
    async fn closing_reports_the_exit_the_way_the_real_host_does() {
        let (terminals, observer) = host();
        let id = SessionId::new("t1");
        terminals
            .open(id.clone(), TerminalSpec::new("tests", "bash", "/tmp"))
            .await
            .expect("the fake succeeds by default");

        terminals.close(&id).await.expect("it is open");
        assert_eq!(terminals.closed(), std::slice::from_ref(&id));
        assert_eq!(observer.exit_of(&id), Some(None));
        assert_eq!(terminals.open_count(), 0);
    }

    #[tokio::test]
    async fn an_injected_failure_keeps_its_classification() {
        let (terminals, _) = host();
        terminals.fail_with(ErrorClass::Retryable);

        let error = terminals
            .open(SessionId::new("t1"), TerminalSpec::new("t", "bash", "/tmp"))
            .await
            .expect_err("the fake was told to fail");
        assert_eq!(error.class(), ErrorClass::Retryable);
    }

    #[tokio::test]
    async fn output_reaches_the_observer_in_order() {
        let (terminals, observer) = host();
        let id = SessionId::new("t1");
        terminals.emit(&id, "one ");
        terminals.emit(&id, "two");
        assert_eq!(observer.output(&id), "one two");
    }
}
