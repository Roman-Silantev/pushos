//! The port for sessions PushOS did not start.
//!
//! Deliberately small, and deliberately not the terminal port. PushOS owns the
//! terminals it runs: it knows when they exit and with what status. It owns
//! none of this, so it can only ask what is there, read what is on screen and
//! type. Pretending the two were the same interface would mean promising
//! things about these that cannot be kept.
//!
//! Even a session this opens is not PushOS's. It asks a program that keeps
//! sessions running by themselves to start one, and that program keeps it
//! whether or not PushOS is still running.

use std::path::PathBuf;

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

    /// Presses a key in a session, without sending a line.
    ///
    /// Typing a line and pressing a key are different acts and the terminal
    /// treats them differently: a line is submitted, a key is a key. Accepting
    /// a suggestion is Tab and nothing else, and a Tab followed by a return
    /// would accept it and send it, which is not the same instruction.
    ///
    /// Where the host can only deliver a key to whatever is in front, this
    /// brings the window to the front first. That is visible and deliberate
    /// rather than hidden.
    async fn press(&self, session: &AttachedId, key: Key) -> Result<(), AttachError>;

    /// Brings a session's window to the front, opening one onto it when a
    /// session that outlives its windows has none.
    async fn focus(&self, session: &AttachedId) -> Result<(), AttachError>;

    /// Starts a session under a name, or finds the one already kept under it.
    ///
    /// Finding rather than failing, so one pad both starts a worker and comes
    /// back to it: the second press is the same request as the first.
    async fn open(&self, request: &OpenSession) -> Result<Opened, AttachError>;

    /// Stops a session's process, keeping the conversation for later.
    ///
    /// Only for a session whose keeper can give it back: see
    /// [`Reach::Dispatching`](crate::attached::Reach::Dispatching). `false`
    /// when this session is not one of those, or was not running anyway.
    /// Asking for it again, by instruction or by opening a window onto it,
    /// starts it where it left off.
    async fn put_away(&self, session: &AttachedId) -> Result<bool, AttachError>;

    /// What this adapter watches, for the log and for `doctor`.
    fn describe(&self) -> &str;
}

/// A session to start, or to find if it is already running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenSession {
    /// What it is kept and called by. See
    /// [`is_session_name`](crate::attached::is_session_name).
    pub name: String,
    /// Where it starts. The home directory when not given.
    pub directory: Option<PathBuf>,
    /// Who keeps it running.
    pub keeper: Keeper,
    /// What to type into its shell once it has started, such as `claude`.
    ///
    /// Typed rather than run in its place, so a session outlives the program:
    /// when an agent exits, what is left is a shell in the same folder rather
    /// than a window that closed.
    pub command: Option<String>,
}

/// Who keeps a session running when PushOS is not looking at it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Keeper {
    /// A terminal multiplexer, which keeps a screen the operator can take over
    /// at any time. Every session kept this way is a program running.
    #[default]
    Terminal,
    /// The coding agent itself, which keeps the conversation and needs a
    /// process only while it is working. One put away costs nothing until it
    /// is asked for again.
    Agent,
}

/// A session that was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    /// The session.
    pub id: AttachedId,
    /// Whether it was started now, rather than found already running.
    pub started: bool,
}

/// A key a pad can press in a session.
///
/// A small, named set rather than arbitrary key codes. These are the keys a
/// coding session actually asks for, and every one of them has a meaning an
/// operator can read off a pad; a general key injector would be a keyboard
/// with none of a keyboard's advantages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// Accepts what is being suggested.
    Tab,
    /// Submits what is there.
    Enter,
    /// Dismisses, or interrupts.
    Escape,
    /// Moves through a list of choices.
    Up,
    /// Moves through a list of choices.
    Down,
}

impl Key {
    /// How it is written in configuration.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tab => "tab",
            Self::Enter => "enter",
            Self::Escape => "escape",
            Self::Up => "up",
            Self::Down => "down",
        }
    }

    /// Every key a pad can press, for validation and documentation.
    pub const ALL: [Self; 5] = [Self::Tab, Self::Enter, Self::Escape, Self::Up, Self::Down];
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Key {
    type Err = UnknownKey;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        // `return` and `esc` are what people call these, so they are spellings
        // of the same key rather than keys PushOS does not have.
        let wanted = match text.trim().to_lowercase().as_str() {
            "return" | "newline" => "enter".to_owned(),
            "esc" => "escape".to_owned(),
            other => other.to_owned(),
        };

        Self::ALL
            .into_iter()
            .find(|key| key.as_str() == wanted)
            .ok_or(UnknownKey {
                named: text.to_owned(),
            })
    }
}

/// A key PushOS does not know how to press.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{named}` is not a key PushOS can press; it has tab, enter, escape, up and down")]
pub struct UnknownKey {
    /// What was written.
    pub named: String,
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

    /// PushOS can see the session but has no way to type into it.
    ///
    /// Its own kind because the fix is the operator's to make, and specific:
    /// answer it where it is, or run the next one somewhere PushOS can reach.
    #[error(
        "PushOS can see `{session}` in {host} but cannot type into it; answer it there, \
         or run it in tmux or Terminal to drive it from the Push"
    )]
    Unreachable {
        /// Which one was asked for.
        session: AttachedId,
        /// What it is running in.
        host: String,
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
            Self::Gone { .. } | Self::Unreachable { .. } | Self::Unavailable { .. } => {
                ErrorClass::Validation
            }
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
