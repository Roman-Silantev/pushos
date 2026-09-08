//! An agent backend that runs nothing.
//!
//! Satisfies the same port as the real adapter, so a test exercises the
//! production path rather than a parallel one, and PushOS can be developed with
//! no agent provider installed and no account signed in.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pushos_domain::agent::AgentState;
use pushos_domain::error::ErrorClass;
use pushos_domain::ids::{ProviderName, SessionId};
use pushos_domain::ports::{
    AgentBackend, AgentCapabilities, AgentError, AgentEvent, AgentObserver, ApprovalId,
    ApprovalOption, SessionHandle, SessionRequest, StopReason,
};

/// Something the operator did to a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentCall {
    /// A session was opened.
    Started {
        /// The role it fills.
        agent: String,
    },
    /// A prompt was sent.
    Prompted {
        /// Which session.
        session: String,
        /// What was said.
        text: String,
    },
    /// Work was interrupted.
    Cancelled {
        /// Which session.
        session: String,
    },
    /// A permission question was answered.
    Answered {
        /// Which session.
        session: String,
        /// Which answer was chosen.
        option: String,
    },
    /// A session was resumed.
    Resumed {
        /// Which session.
        session: String,
    },
    /// A session was ended.
    Stopped {
        /// Which session.
        session: String,
    },
}

/// An agent provider that exists only in memory.
#[derive(Debug, Clone)]
pub struct FakeAgent {
    provider: ProviderName,
    capabilities: Arc<Mutex<AgentCapabilities>>,
    calls: Arc<Mutex<Vec<AgentCall>>>,
    observer: Arc<Mutex<Option<Arc<dyn AgentObserver>>>>,
    fail_with: Arc<Mutex<Option<ErrorClass>>>,
    next_session: Arc<Mutex<u64>>,
}

impl FakeAgent {
    /// Builds a provider that can do everything.
    pub fn new(provider: impl Into<ProviderName>) -> Self {
        Self {
            provider: provider.into(),
            capabilities: Arc::new(Mutex::new(AgentCapabilities {
                resume: true,
                cancel: true,
                permissions: true,
                terminal: false,
                worktrees: false,
                models: Vec::new(),
            })),
            calls: Arc::new(Mutex::new(Vec::new())),
            observer: Arc::new(Mutex::new(None)),
            fail_with: Arc::new(Mutex::new(None)),
            next_session: Arc::new(Mutex::new(0)),
        }
    }

    /// Sets what the provider claims it can do.
    pub fn set_capabilities(&self, capabilities: AgentCapabilities) {
        if let Ok(mut current) = self.capabilities.lock() {
            *current = capabilities;
        }
    }

    /// Sends session activity somewhere, as a real provider would.
    pub fn report_to(&self, observer: Arc<dyn AgentObserver>) {
        if let Ok(mut current) = self.observer.lock() {
            *current = Some(observer);
        }
    }

    /// Makes every subsequent call fail with the given classification.
    pub fn fail_with(&self, class: ErrorClass) {
        if let Ok(mut current) = self.fail_with.lock() {
            *current = Some(class);
        }
    }

    /// Everything the provider was asked to do, in order.
    pub fn calls(&self) -> Vec<AgentCall> {
        self.calls
            .lock()
            .map(|calls| calls.clone())
            .unwrap_or_default()
    }

    /// Pretends the session said something.
    pub fn say(&self, session: &SessionId, text: &str) {
        self.emit(
            session,
            AgentEvent::Said {
                text: text.to_owned(),
            },
        );
    }

    /// Pretends the session moved to a state.
    pub fn move_to(&self, session: &SessionId, state: AgentState) {
        self.emit(session, AgentEvent::StateChanged { state });
    }

    /// Pretends the session is asking to be allowed to do something.
    pub fn ask_permission(&self, session: &SessionId, question: &str) {
        self.emit(
            session,
            AgentEvent::AskedPermission {
                request: ApprovalId(format!("{session}-request")),
                question: question.to_owned(),
                options: vec![
                    ApprovalOption {
                        id: "allow".to_owned(),
                        label: "Allow".to_owned(),
                        allows: true,
                    },
                    ApprovalOption {
                        id: "reject".to_owned(),
                        label: "Reject".to_owned(),
                        allows: false,
                    },
                ],
            },
        );
    }

    /// Pretends the session ended.
    pub fn finish(&self, session: &SessionId, reason: StopReason) {
        self.emit(session, AgentEvent::Ended { reason });
    }

    fn emit(&self, session: &SessionId, event: AgentEvent) {
        let observer = self.observer.lock().ok().and_then(|guard| guard.clone());
        if let Some(observer) = observer {
            observer.observe(session, event);
        }
    }

    fn record(&self, call: AgentCall) -> Result<(), AgentError> {
        self.check()?;
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(call);
        }
        Ok(())
    }

    fn check(&self) -> Result<(), AgentError> {
        let class = self.fail_with.lock().ok().and_then(|guard| *guard);
        match class {
            None => Ok(()),
            Some(class) => Err(AgentError::backend(
                "the fake agent was told to fail",
                class,
                std::io::Error::other("injected failure"),
            )),
        }
    }
}

#[async_trait]
impl AgentBackend for FakeAgent {
    fn provider(&self) -> ProviderName {
        self.provider.clone()
    }

    async fn capabilities(&self) -> Result<AgentCapabilities, AgentError> {
        self.check()?;
        Ok(self
            .capabilities
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default())
    }

    async fn start(&self, request: SessionRequest) -> Result<SessionHandle, AgentError> {
        self.record(AgentCall::Started {
            agent: request.agent.to_string(),
        })?;

        let ordinal = {
            let mut next = self
                .next_session
                .lock()
                .map_err(|_| AgentError::Unavailable {
                    provider: self.provider.clone(),
                })?;
            *next += 1;
            *next
        };
        let id = SessionId::new(format!("{}-{}-{ordinal}", self.provider, request.agent));

        Ok(SessionHandle {
            id,
            provider_session: Some(format!("provider-{ordinal}")),
            provider: self.provider.clone(),
        })
    }

    async fn resume(&self, handle: &SessionHandle) -> Result<(), AgentError> {
        self.record(AgentCall::Resumed {
            session: handle.id.to_string(),
        })
    }

    async fn prompt(&self, handle: &SessionHandle, text: &str) -> Result<(), AgentError> {
        self.record(AgentCall::Prompted {
            session: handle.id.to_string(),
            text: text.to_owned(),
        })
    }

    async fn cancel(&self, handle: &SessionHandle) -> Result<(), AgentError> {
        self.record(AgentCall::Cancelled {
            session: handle.id.to_string(),
        })
    }

    async fn answer(
        &self,
        handle: &SessionHandle,
        _request: &ApprovalId,
        option: &str,
    ) -> Result<(), AgentError> {
        self.record(AgentCall::Answered {
            session: handle.id.to_string(),
            option: option.to_owned(),
        })
    }

    async fn stop(&self, handle: &SessionHandle) -> Result<(), AgentError> {
        self.record(AgentCall::Stopped {
            session: handle.id.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(agent: &str) -> SessionRequest {
        SessionRequest {
            agent: pushos_domain::ids::AgentId::new(agent),
            workspace: None,
            cwd: std::path::PathBuf::from("/tmp"),
            objective: String::new(),
        }
    }

    #[tokio::test]
    async fn each_session_gets_its_own_identity() {
        let agent = FakeAgent::new("claude");
        let first = agent.start(request("builder")).await.expect("starts");
        let second = agent.start(request("builder")).await.expect("starts");

        assert_ne!(
            first.id, second.id,
            "two sessions must not share an identity"
        );
        assert_eq!(agent.calls().len(), 2);
    }

    #[tokio::test]
    async fn an_injected_failure_keeps_its_classification_and_records_nothing() {
        let agent = FakeAgent::new("claude");
        agent.fail_with(ErrorClass::Permission);

        let error = agent
            .start(request("builder"))
            .await
            .expect_err("told to fail");
        assert_eq!(error.class(), ErrorClass::Permission);
        assert!(agent.calls().is_empty());
    }

    #[tokio::test]
    async fn reported_activity_reaches_the_observer() {
        #[derive(Debug, Default)]
        struct Collector(Mutex<Vec<AgentEvent>>);

        impl AgentObserver for Collector {
            fn observe(&self, _session: &SessionId, event: AgentEvent) {
                if let Ok(mut seen) = self.0.lock() {
                    seen.push(event);
                }
            }
        }

        let collector = Arc::new(Collector::default());
        let agent = FakeAgent::new("claude");
        agent.report_to(collector.clone());

        let session = SessionId::new("s1");
        agent.say(&session, "working on it");
        agent.finish(&session, StopReason::Completed);

        let seen = collector.0.lock().expect("readable").clone();
        assert_eq!(seen.len(), 2);
        assert!(matches!(seen[0], AgentEvent::Said { .. }));
        assert!(matches!(
            seen[1],
            AgentEvent::Ended {
                reason: StopReason::Completed
            }
        ));
    }
}
