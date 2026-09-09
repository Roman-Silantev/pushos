//! What the runtime must provide for a control socket to be useful.
//!
//! Keeping this a trait means the socket has no idea how PushOS is put
//! together, and the runtime has no idea it is being talked to over a socket.

use async_trait::async_trait;

use crate::protocol::{
    BindingList, EditReport, Failure, FailureKind, PackList, PackReview, SessionList, StatusReport,
    TestReport, Vocabulary, WorkspaceList,
};
use pushos_config::{BindingAddress, BindingSpec, PageSpec};

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

    /// Adds a page, or replaces the one already using that identity.
    async fn add_page(&self, spec: PageSpec) -> Result<EditReport, Failure>;

    /// Removes a page, and everything bound on it.
    async fn remove_page(&self, page: &str) -> Result<EditReport, Failure>;

    /// Runs a configured binding's action once.
    async fn test(&self, address: BindingAddress) -> Result<TestReport, Failure>;

    /// Every live agent and terminal session.
    ///
    /// Defaulted to nothing: a PushOS with no agent roles configured and no
    /// terminals open has no sessions, and that is not a failure.
    async fn sessions(&self) -> Result<SessionList, Failure> {
        Ok(SessionList::default())
    }

    /// Every configured project, and which one is in effect.
    ///
    /// Defaulted to nothing: a PushOS with no projects configured has none,
    /// and that is not a failure.
    async fn workspaces(&self) -> Result<WorkspaceList, Failure> {
        Ok(WorkspaceList::default())
    }

    /// Re-reads the configuration from disk.
    async fn reload(&self) -> Result<StatusReport, Failure>;

    /// Every pack on offer, and which of them are installed.
    ///
    /// Defaulted to nothing: a PushOS with nowhere to read packs from has none
    /// to offer, and that is not a failure.
    async fn packs(&self) -> Result<PackList, Failure> {
        Ok(PackList::default())
    }

    /// What installing one would add, and what it would ask for.
    async fn review_pack(&self, pack: &str) -> Result<PackReview, Failure> {
        Err(Failure::new(
            FailureKind::NotFound,
            format!("no pack called `{pack}` is on offer"),
        ))
    }

    /// Installs a pack, granting exactly what its review asked for.
    ///
    /// `granting` is what the operator agreed to, and must match what
    /// [`Self::review_pack`] named. A client cannot agree on their behalf to
    /// something it never showed them, and a list that has drifted since the
    /// review is a list they did not read.
    async fn install_pack(
        &self,
        pack: &str,
        _granting: &[pushos_domain::permissions::Permission],
    ) -> Result<EditReport, Failure> {
        Err(Failure::new(
            FailureKind::NotFound,
            format!("no pack called `{pack}` is on offer"),
        ))
    }
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
