//! Terminals PushOS did not open, without any that exist.
//!
//! Stands in for whatever can see other terminals, so the sessions namespace
//! can be exercised on a machine with none open and no permission to ask.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pushos_domain::attached::{Activity, Attached};
use pushos_domain::error::ErrorClass;
use pushos_domain::ids::AttachedId;
use pushos_domain::ports::{AttachError, AttachedSessions, Key, OpenSession, Opened};

/// What a fake was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionCall {
    /// Everything open was asked for.
    Discovered,
    /// A session's screen was read.
    Read(String),
    /// Something was typed into a session.
    Sent(String, String),
    /// A key was pressed in a session.
    Pressed(String, Key),
    /// A session's window was brought to the front.
    Focused(String),
    /// A session was asked for by name.
    Opened(OpenSession),
    /// A session was put away, keeping its conversation.
    PutAway(String),
}

/// Terminals that exist only in memory.
#[derive(Debug, Clone, Default)]
pub struct FakeAttached {
    open: Arc<Mutex<Vec<Attached>>>,
    screen: Arc<Mutex<String>>,
    calls: Arc<Mutex<Vec<SessionCall>>>,
    refuse: Arc<Mutex<bool>>,
}

impl FakeAttached {
    /// Builds a watcher with nothing open.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a watcher over some sessions, each waiting for the operator.
    pub fn with_sessions<D: Into<String>, T: Into<String>>(
        sessions: impl IntoIterator<Item = (D, T)>,
    ) -> Self {
        Self::doing(
            sessions
                .into_iter()
                .map(|(device, title)| (device, title, Activity::Ready)),
        )
    }

    /// Builds a watcher over some sessions, each doing something.
    pub fn doing<D: Into<String>, T: Into<String>>(
        sessions: impl IntoIterator<Item = (D, T, Activity)>,
    ) -> Self {
        let fake = Self::new();
        if let Ok(mut open) = fake.open.lock() {
            *open = sessions
                .into_iter()
                .map(|(device, title, activity)| {
                    Attached::new(device.into(), title.into(), activity, "Terminal")
                })
                .collect();
        }
        fake
    }

    /// Builds a watcher over sessions described in full.
    pub fn holding(sessions: impl IntoIterator<Item = Attached>) -> Self {
        let fake = Self::new();
        if let Ok(mut open) = fake.open.lock() {
            *open = sessions.into_iter().collect();
        }
        fake
    }

    /// Sets what reading a session returns.
    pub fn showing(&self, screen: &str) {
        if let Ok(mut held) = self.screen.lock() {
            screen.clone_into(&mut held);
        }
    }

    /// Closes a session, as an operator closing a window would.
    pub fn close(&self, device: &str) {
        if let Ok(mut open) = self.open.lock() {
            open.retain(|session| session.id.as_str() != device);
        }
    }

    /// Makes every subsequent call fail as a refused permission would.
    pub fn refuse(&self) {
        if let Ok(mut refuse) = self.refuse.lock() {
            *refuse = true;
        }
    }

    /// Everything it was asked to do, in order.
    pub fn calls(&self) -> Vec<SessionCall> {
        self.calls.lock().map_or_default(|calls| calls.clone())
    }

    fn record(&self, call: SessionCall) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(call);
        }
    }

    fn check(&self) -> Result<(), AttachError> {
        if self.refuse.lock().is_ok_and(|refuse| *refuse) {
            return Err(AttachError::NotPermitted {
                application: "the fake".to_owned(),
            });
        }
        Ok(())
    }

    fn find(&self, session: &AttachedId) -> Result<Attached, AttachError> {
        let open = self.open.lock().map_err(|_| poisoned())?;
        open.iter()
            .find(|held| held.id == *session)
            .cloned()
            .ok_or_else(|| AttachError::Gone {
                session: session.clone(),
            })
    }

    /// Finds a session that can be typed into, as a real host would refuse one
    /// it can only watch.
    fn reachable(&self, session: &AttachedId) -> Result<(), AttachError> {
        let found = self.find(session)?;
        if found.can_type() {
            Ok(())
        } else {
            Err(AttachError::Unreachable {
                session: found.id,
                host: found.host,
            })
        }
    }
}

#[async_trait]
impl AttachedSessions for FakeAttached {
    async fn discover(&self) -> Result<Vec<Attached>, AttachError> {
        self.check()?;
        self.record(SessionCall::Discovered);
        Ok(self.open.lock().map_or_default(|open| open.clone()))
    }

    async fn put_away(&self, session: &AttachedId) -> Result<bool, AttachError> {
        self.check()?;
        self.record(SessionCall::PutAway(session.to_string()));
        let found = self
            .open
            .lock()
            .map_or_default(|open| open.iter().find(|held| &held.id == session).cloned());
        Ok(found.is_some_and(|held| held.can_be_put_away()))
    }

    async fn read(&self, session: &AttachedId, _most: usize) -> Result<String, AttachError> {
        self.check()?;
        self.reachable(session)?;
        self.record(SessionCall::Read(session.to_string()));
        Ok(self.screen.lock().map_or_default(|screen| screen.clone()))
    }

    async fn send(&self, session: &AttachedId, text: &str) -> Result<(), AttachError> {
        self.check()?;
        self.reachable(session)?;
        self.record(SessionCall::Sent(session.to_string(), text.to_owned()));
        Ok(())
    }

    async fn press(&self, session: &AttachedId, key: Key) -> Result<(), AttachError> {
        self.check()?;
        self.reachable(session)?;
        self.record(SessionCall::Pressed(session.to_string(), key));
        Ok(())
    }

    async fn focus(&self, session: &AttachedId) -> Result<(), AttachError> {
        self.check()?;
        self.find(session)?;
        self.record(SessionCall::Focused(session.to_string()));
        Ok(())
    }

    async fn open(&self, request: &OpenSession) -> Result<Opened, AttachError> {
        self.check()?;
        self.record(SessionCall::Opened(request.clone()));
        let mut open = self.open.lock().map_err(|_| poisoned())?;

        if let Some(held) = open
            .iter()
            .find(|held| held.name.as_deref() == Some(request.name.as_str()))
        {
            return Ok(Opened {
                id: held.id.clone(),
                started: false,
            });
        }

        let id = format!("/dev/ttys{:03}", 900 + open.len());
        open.push(Attached::new(id.clone(), "", Activity::Quiet, "tmux").named(&request.name));
        Ok(Opened {
            id: AttachedId::new(id),
            started: true,
        })
    }

    fn describe(&self) -> &'static str {
        "a fake"
    }
}

fn poisoned() -> AttachError {
    AttachError::backend(
        "the fake is poisoned",
        ErrorClass::ComponentFailure,
        std::io::Error::other("poisoned"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_fake_reports_the_sessions_it_was_given() {
        let fake = FakeAttached::with_sessions([("/dev/ttys003", "Sprint 2")]);
        let open = fake.discover().await.expect("the fake succeeds");
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].label(), "Sprint 2");
    }

    #[tokio::test]
    async fn a_closed_session_is_gone_rather_than_broken() {
        let fake = FakeAttached::with_sessions([("/dev/ttys003", "Sprint 2")]);
        fake.close("/dev/ttys003");

        let error = fake
            .send(&AttachedId::new("/dev/ttys003"), "hello")
            .await
            .expect_err("it was closed");
        assert!(matches!(error, AttachError::Gone { .. }));
    }

    #[tokio::test]
    async fn asking_twice_for_the_same_name_finds_the_first() {
        let fake = FakeAttached::new();
        let request = OpenSession {
            name: "client-1".to_owned(),
            keeper: pushos_domain::ports::Keeper::Terminal,
            directory: None,
            command: Some("claude".to_owned()),
        };
        let first = fake.open(&request).await.expect("opens");
        let second = fake.open(&request).await.expect("finds");
        assert!(first.started && !second.started);
        assert_eq!(first.id, second.id);
    }

    #[tokio::test]
    async fn a_session_it_can_only_watch_refuses_typing_the_way_a_host_would() {
        let fake = FakeAttached::holding([Attached::new(
            "claude:0d2c",
            "panel",
            Activity::Ready,
            "Visual Studio Code",
        )
        .watched_only()]);
        let error = fake
            .send(&AttachedId::new("claude:0d2c"), "hello")
            .await
            .expect_err("nothing to type into");
        assert!(matches!(error, AttachError::Unreachable { .. }));
    }
}
