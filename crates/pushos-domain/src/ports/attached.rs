//! The port for sessions PushOS did not start.
//!
//! Deliberately small, and deliberately not the terminal port. PushOS owns the
//! terminals it runs: it knows when they exit and with what status. It owns
//! none of this, so it can only ask what is there, read what is on screen and
//! type. Pretending the two were the same interface would mean promising
//! things about these that cannot be kept.

use async_trait::async_trait;

use crate::attached::Attached;
use crate::ids::AttachedId;

/// Sessions running in terminals PushOS did not open.
#[async_trait]
pub trait AttachedSessions: Send + Sync + std::fmt::Debug {
    /// Everything PushOS can currently see.
    ///
    /// Asked for repeatedly rather than subscribed to, because nothing tells
    /// PushOS when a window is opened or closed.
    async fn discover(&self) -> Result<Vec<Attached>, AttachError>;

    /// The last of what a session has on screen.
    ///
    /// Bounded, because a scrollback is unbounded and a display is 160 pixels
    /// tall. What comes back is whatever the terminal holds, including the
    /// escape sequences a program drew it with.
    async fn read(&self, session: &AttachedId, most: usize) -> Result<String, AttachError>;

    /// Types into a session, exactly as written.
    ///
    /// The operator is holding the keyboard for these sessions too, so this is
    /// for the things a pad is better at: answering the same prompt in the
    /// eighth window, or interrupting something.
    async fn send(&self, session: &AttachedId, text: &str) -> Result<(), AttachError>;

    /// Brings a session's window to the front.
    async fn focus(&self, session: &AttachedId) -> Result<(), AttachError>;

    /// What this adapter watches, for the log and for `doctor`.
    fn describe(&self) -> &str;
}

/// Why a session could not be found, read or typed into.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AttachError {
    /// The operator has not allowed PushOS to control the application.
    ///
    /// Its own kind because it is the only one they can do something about,
    /// and the something is specific.
    #[error(
        "PushOS is not allowed to control {application}; allow it in System Settings \
         under Privacy and Security, Automation"
    )]
    NotPermitted {
        /// Which application refused.
        application: String,
    },

    /// The session is not there any more.
    ///
    /// Not a fault: a window the operator closed is a fact, and a pad pointing
    /// at it should say so rather than fail.
    #[error("no session is on `{session}` any more")]
    Gone {
        /// Which one was asked for.
        session: AttachedId,
    },

    /// Nothing here can see other terminals.
    #[error("{context}")]
    Unavailable {
        /// What is missing, and what would fix it.
        context: String,
    },

    /// The application reported a fault.
    #[error("{context}")]
    Backend {
        /// What PushOS was attempting.
        context: String,
        /// How a caller should react.
        class: crate::error::ErrorClass,
        /// The originating fault.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl AttachError {
    /// How a caller should react.
    pub const fn class(&self) -> crate::error::ErrorClass {
        use crate::error::ErrorClass;
        match self {
            Self::NotPermitted { .. } => ErrorClass::Permission,
            Self::Gone { .. } | Self::Unavailable { .. } => ErrorClass::Validation,
            Self::Backend { class, .. } => *class,
        }
    }

    /// Reports that nothing here can see other terminals.
    pub fn unavailable(context: impl Into<String>) -> Self {
        Self::Unavailable {
            context: context.into(),
        }
    }

    /// Wraps an application fault with the context that makes it actionable.
    pub fn backend(
        context: impl Into<String>,
        class: crate::error::ErrorClass,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Backend {
            context: context.into(),
            class,
            source: Box::new(source),
        }
    }
}
