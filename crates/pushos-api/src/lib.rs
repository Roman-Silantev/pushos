//! The local control socket.
//!
//! PushOS Studio is a separate process. It talks to a running PushOS over a
//! Unix socket rather than embedding the runtime, so closing Studio cannot stop
//! PushOS, and a crash in Studio cannot take the surface down.
//!
//! The socket is local, user-owned and unreadable by anyone else. There is no
//! network listener, and there never should be.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod client;
pub mod endpoint;
mod plane;
pub mod protocol;
mod server;

pub use client::{ClientError, ControlClient};
pub use endpoint::{SOCKET_FILE, SOCKET_MODE, default_socket, socket_in};
pub use plane::ControlPlane;
pub use protocol::{
    BindingList, ControlInfo, EditReport, Failure, FailureKind, GridPosition, PROTOCOL_VERSION,
    PageInfo, ProviderInfo, Request, Response, StatusReport, TestReport, Vocabulary,
};
pub use server::{ControlServer, ServerError};
