//! Talking to a running PushOS.
//!
//! Used by the command line and by Studio's backend, so both exercise the same
//! protocol rather than one of them growing a private path.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;

use crate::protocol::{Failure, FailureKind, PROTOCOL_VERSION, Request, Response};

/// How long a request may take before the client gives up.
///
/// A control request either answers immediately or is running an action the
/// operator asked to test, which has its own budget inside the runtime.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

/// A connection to a running PushOS.
#[derive(Debug)]
pub struct ControlClient {
    writer: OwnedWriteHalf,
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
}

impl ControlClient {
    /// Connects to the socket at `path`.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, ClientError> {
        let path = path.as_ref();
        let stream =
            UnixStream::connect(path)
                .await
                .map_err(|source| ClientError::Unreachable {
                    path: path.to_path_buf(),
                    source,
                })?;

        let (reader, writer) = stream.into_split();
        Ok(Self {
            writer,
            lines: BufReader::new(reader).lines(),
        })
    }

    /// Connects to the default socket.
    pub async fn connect_default() -> Result<Self, ClientError> {
        let path = crate::endpoint::default_socket().ok_or(ClientError::NoSocketPath)?;
        Self::connect(path).await
    }

    /// Sends a request and waits for its answer.
    pub async fn send(&mut self, request: &Request) -> Result<Response, ClientError> {
        let mut line = serde_json::to_vec(request).map_err(ClientError::Encode)?;
        line.push(b'\n');

        tokio::time::timeout(REQUEST_TIMEOUT, async {
            self.writer.write_all(&line).await?;
            self.writer.flush().await
        })
        .await
        .map_err(|_| ClientError::TimedOut)?
        .map_err(ClientError::Transport)?;

        let answer = tokio::time::timeout(REQUEST_TIMEOUT, self.lines.next_line())
            .await
            .map_err(|_| ClientError::TimedOut)?
            .map_err(ClientError::Transport)?
            .ok_or(ClientError::Closed)?;

        serde_json::from_str(&answer).map_err(ClientError::Decode)
    }

    /// Checks that the runtime speaks a protocol this build understands.
    ///
    /// Worth doing before any edit: a mismatched runtime might read a request
    /// differently from how it was meant.
    pub async fn handshake(&mut self) -> Result<crate::protocol::StatusReport, ClientError> {
        match self.send(&Request::Status).await? {
            Response::Status(status) if status.protocol == PROTOCOL_VERSION => Ok(status),
            Response::Status(status) => Err(ClientError::ProtocolMismatch {
                runtime: status.protocol,
                client: PROTOCOL_VERSION,
            }),
            Response::Failed(failure) => Err(ClientError::Refused(failure)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }
}

/// Why talking to PushOS failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// Nothing is listening.
    #[error("PushOS is not running, or its control socket is not at `{path}`")]
    Unreachable {
        /// The socket that was tried.
        path: PathBuf,
        /// What the operating system reported.
        #[source]
        source: std::io::Error,
    },

    /// There is nowhere a socket could be.
    #[error("could not work out where the control socket lives; set XDG_DATA_HOME or HOME")]
    NoSocketPath,

    /// The runtime speaks a different protocol.
    #[error(
        "this build speaks control protocol {client} but PushOS speaks {runtime}; \
         update whichever is older"
    )]
    ProtocolMismatch {
        /// What the runtime speaks.
        runtime: u32,
        /// What this build speaks.
        client: u32,
    },

    /// The runtime declined the request.
    #[error("{}", .0.message)]
    Refused(Failure),

    /// The answer was not the kind expected.
    #[error("PushOS answered with something unexpected: {0}")]
    Unexpected(String),

    /// The connection ended before an answer arrived.
    #[error("PushOS closed the connection without answering")]
    Closed,

    /// No answer arrived in time.
    #[error("PushOS did not answer in time")]
    TimedOut,

    /// The request could not be encoded.
    #[error("could not encode the request")]
    Encode(#[source] serde_json::Error),

    /// The answer could not be read.
    #[error("could not read the answer")]
    Decode(#[source] serde_json::Error),

    /// The socket failed.
    #[error("the control connection failed")]
    Transport(#[source] std::io::Error),
}

impl ClientError {
    /// Whether the failure simply means PushOS is not running.
    pub const fn is_not_running(&self) -> bool {
        matches!(self, Self::Unreachable { .. })
    }

    /// The itemised problems, when a configuration was rejected.
    pub fn problems(&self) -> &[String] {
        match self {
            Self::Refused(failure) => &failure.problems,
            _ => &[],
        }
    }

    /// Whether the failure was the runtime declining rather than a transport
    /// fault.
    pub const fn kind(&self) -> Option<FailureKind> {
        match self {
            Self::Refused(failure) => Some(failure.kind),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connecting_to_nothing_says_pushos_is_not_running() {
        let error = ControlClient::connect("/tmp/pushos-definitely-not-here.sock")
            .await
            .expect_err("nothing is listening");

        assert!(error.is_not_running());
        assert!(error.to_string().contains("not running"));
    }

    #[test]
    fn a_refusal_carries_its_problems_through() {
        let error = ClientError::Refused(
            Failure::new(FailureKind::Rejected, "not usable")
                .with_problems(vec!["binding 1: unknown control".to_owned()]),
        );

        assert_eq!(error.kind(), Some(FailureKind::Rejected));
        assert_eq!(error.problems().len(), 1);
        assert!(!error.is_not_running());
    }

    #[test]
    fn a_protocol_mismatch_says_which_side_is_older() {
        let error = ClientError::ProtocolMismatch {
            runtime: 2,
            client: 1,
        };
        let message = error.to_string();
        assert!(message.contains('1') && message.contains('2'));
    }
}
