//! Terminals PushOS did not open, without any that exist.
//!
//! Stands in for whatever can see other terminals, so the sessions namespace
//! can be exercised on a machine with none open and no permission to ask.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pushos_domain::attached::Attached;
use pushos_domain::error::ErrorClass;
use pushos_domain::ids::AttachedId;
use pushos_domain::ports::{AttachError, AttachedSessions};

/// What a fake was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionCall {
    /// Everything open was asked for.
    Discovered,
    /// A session's screen was read.
    Read(String),
    /// Something was typed into a session.
    Sent(String, String),
    /// A session's window was brought to the front.
    Focused(String),
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

    /// Builds a watcher over some sessions.
    pub fn with_sessions(sessions: impl IntoIterator<Item = (&'static str, &'static str)>) -> Self {
        let fake = Self::new();
        if let Ok(mut open) = fake.open.lock() {
            *open = sessions
                .into_iter()
                .map(|(device, title)| Attached {
                    id: AttachedId::new(device),
                    title: title.to_owned(),
                    busy: true,
                })
                .collect();
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

    fn find(&self, session: &AttachedId) -> Result<(), AttachError> {
        let open = self.open.lock().map_err(|_| {
            AttachError::backend(
                "the fake is poisoned",
                ErrorClass::ComponentFailure,
                std::io::Error::other("poisoned"),
            )
        })?;
        if open.iter().any(|held| held.id == *session) {
            Ok(())
        } else {
            Err(AttachError::Gone {
                session: session.clone(),
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

    async fn read(&self, session: &AttachedId, _most: usize) -> Result<String, AttachError> {
        self.check()?;
        self.find(session)?;
        self.record(SessionCall::Read(session.to_string()));
        Ok(self.screen.lock().map_or_default(|screen| screen.clone()))
    }

    async fn send(&self, session: &AttachedId, text: &str) -> Result<(), AttachError> {
        self.check()?;
        self.find(session)?;
        self.record(SessionCall::Sent(session.to_string(), text.to_owned()));
        Ok(())
    }

    async fn focus(&self, session: &AttachedId) -> Result<(), AttachError> {
        self.check()?;
        self.find(session)?;
        self.record(SessionCall::Focused(session.to_string()));
        Ok(())
    }

    fn describe(&self) -> &'static str {
        "a fake"
    }
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
}
