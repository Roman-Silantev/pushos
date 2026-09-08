//! What the runtime must provide for a control socket to be useful.
//!
//! Keeping this a trait means the socket has no idea how PushOS is put
//! together, and the runtime has no idea it is being talked to over a socket.

use async_trait::async_trait;

use crate::protocol::{
    BindingList, EditReport, Failure, SessionList, StatusReport, TestReport, Vocabulary,
};
use pushos_config::{BindingAddress, BindingSpec};

/// The operations a control client can ask for.
#[async_trait]
pub trait ControlPlane: Send + Sync + std::fmt::Debug {
    /// How the runtime is doing.
    async fn status(&self) -> StatusReport;

    /// Every control, gesture, page and action available.
    async fn describe(&self) -> Vocabulary;

    /// Every binding currently configured.
    async fn bindings(&self) -> Result<BindingList, Failure>;

    /// Adds a binding, or replaces the one at the same address.
    async fn bind(&self, spec: BindingSpec) -> Result<EditReport, Failure>;

    /// Removes the binding at an address.
    async fn unbind(&self, address: BindingAddress) -> Result<EditReport, Failure>;

    /// Runs a configured binding's action once.
    async fn test(&self, address: BindingAddress) -> Result<TestReport, Failure>;

    /// Every live agent and terminal session.
    ///
    /// Defaulted to nothing: a PushOS with no agent roles configured and no
    /// terminals open has no sessions, and that is not a failure.
    async fn sessions(&self) -> Result<SessionList, Failure> {
        Ok(SessionList::default())
    }

    /// Re-reads the configuration from disk.
    async fn reload(&self) -> Result<StatusReport, Failure>;
}

/// Somewhere the live sessions can be read from.
///
/// Agents and terminals answer this separately, so a PushOS with only one of
/// them configured carries only that one and neither knows about the other.
#[async_trait]
pub trait SessionSource: Send + Sync + std::fmt::Debug {
    /// Every session this source knows about, oldest first.
    async fn sessions(&self) -> Vec<crate::protocol::SessionInfo>;
}
