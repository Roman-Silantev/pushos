//! Serving the control socket.
//!
//! One connection at a time is handled per task, each request read and answered
//! before the next, so a client cannot interleave two edits.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt as _, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, info, warn};

use crate::endpoint::SOCKET_MODE;
use crate::plane::ControlPlane;
use crate::protocol::{Failure, FailureKind, Request, Response};

/// The longest a socket path may be.
///
/// The address struct the kernel takes has a fixed-size path field: 104 bytes
/// on macOS, 108 on Linux, including the terminator. Exceeding it fails with a
/// message about `SUN_LEN` that says nothing useful, so PushOS checks first and
/// explains what is actually wrong.
const MAX_SOCKET_PATH: usize = 100;

/// The longest request line accepted.
///
/// Bounded so a client cannot make PushOS allocate without limit; a binding
/// with its parameters is far smaller than this.
const MAX_REQUEST_BYTES: u64 = 64 * 1024;

/// A listening control socket.
///
/// Removing the socket file on drop means a stale file never makes a client
/// think PushOS is running when it is not.
#[derive(Debug)]
pub struct ControlServer {
    listener: UnixListener,
    path: PathBuf,
}

impl ControlServer {
    /// Binds the socket, replacing a stale one left by a previous run.
    pub fn bind(path: impl Into<PathBuf>) -> Result<Self, ServerError> {
        let path = path.into();

        let length = path.as_os_str().len();
        if length > MAX_SOCKET_PATH {
            return Err(ServerError::new(
                path.clone(),
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "the path is {length} characters but a Unix socket path may be at \
                         most {MAX_SOCKET_PATH}; set XDG_DATA_HOME to somewhere shorter"
                    ),
                ),
            ));
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| ServerError::new(parent.to_path_buf(), source))?;
        }

        // A socket file may be one of two things: left behind by a process that
        // was killed, or held by a PushOS that is running right now. Removing
        // it without asking is how a second copy silently takes the socket and
        // then fights the first one for the Push 2.
        if path.exists() {
            if is_listening(&path) {
                return Err(ServerError::new(
                    path.clone(),
                    std::io::Error::new(
                        std::io::ErrorKind::AddrInUse,
                        "PushOS is already running; stop it before starting another",
                    ),
                ));
            }
            std::fs::remove_file(&path).map_err(|source| ServerError::new(path.clone(), source))?;
        }

        let listener =
            UnixListener::bind(&path).map_err(|source| ServerError::new(path.clone(), source))?;
        restrict(&path).map_err(|source| ServerError::new(path.clone(), source))?;

        info!(?path, "control socket listening");
        Ok(Self { listener, path })
    }

    /// The path the socket is bound to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Accepts connections until `stop` resolves.
    ///
    /// A client that misbehaves loses its own connection and nothing else.
    pub async fn serve(self, plane: Arc<dyn ControlPlane>, stop: impl Future<Output = ()>) {
        let mut stop = std::pin::pin!(stop);

        loop {
            tokio::select! {
                biased;

                () = &mut stop => break,

                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let plane = Arc::clone(&plane);
                        // Connections are short and serialised per client; a
                        // failure in one must not end the listener.
                        tokio::spawn(async move {
                            if let Err(error) = converse(stream, plane.as_ref()).await {
                                debug!(%error, "control connection ended");
                            }
                        });
                    }
                    Err(error) => warn!(%error, "could not accept a control connection"),
                },
            }
        }

        debug!("control socket stopped");
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

/// Reads requests and writes answers until the client goes away.
async fn converse(stream: UnixStream, plane: &dyn ControlPlane) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    // The budget is per request rather than per connection: it is reset after
    // each answer, so a long session is fine but one enormous line is not.
    let mut lines = BufReader::new(reader.take(MAX_REQUEST_BYTES)).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => answer(request, plane).await,
            Err(error) => Response::Failed(Failure::new(
                FailureKind::Malformed,
                format!("could not read the request: {error}"),
            )),
        };

        let mut encoded = serde_json::to_vec(&response).unwrap_or_else(|error| {
            // Serialising our own response type cannot fail in practice;
            // answering with something valid beats dropping the connection.
            serde_json::to_vec(&Response::Failed(Failure::new(
                FailureKind::Internal,
                format!("could not encode the response: {error}"),
            )))
            .unwrap_or_default()
        });
        encoded.push(b'\n');
        writer.write_all(&encoded).await?;
        writer.flush().await?;

        lines.get_mut().get_mut().set_limit(MAX_REQUEST_BYTES);
    }

    Ok(())
}

async fn answer(request: Request, plane: &dyn ControlPlane) -> Response {
    match request {
        Request::Status => Response::Status(plane.status().await),
        Request::Describe => Response::Vocabulary(Box::new(plane.describe().await)),
        Request::Bindings => match plane.bindings().await {
            Ok(bindings) => Response::Bindings(bindings),
            Err(failure) => Response::Failed(failure),
        },
        Request::Bind { spec } => match plane.bind(*spec).await {
            Ok(report) => Response::Edited(report),
            Err(failure) => Response::Failed(failure),
        },
        Request::Unbind { address } => match plane.unbind(address).await {
            Ok(report) => Response::Edited(report),
            Err(failure) => Response::Failed(failure),
        },
        Request::AddPage { spec } => match plane.add_page(spec).await {
            Ok(report) => Response::Edited(report),
            Err(failure) => Response::Failed(failure),
        },
        Request::RemovePage { page } => match plane.remove_page(&page).await {
            Ok(report) => Response::Edited(report),
            Err(failure) => Response::Failed(failure),
        },
        Request::Test { address } => match plane.test(address).await {
            Ok(report) => Response::Tested(report),
            Err(failure) => Response::Failed(failure),
        },
        Request::Sessions => match plane.sessions().await {
            Ok(sessions) => Response::Sessions(sessions),
            Err(failure) => Response::Failed(failure),
        },
        Request::Workspaces => match plane.workspaces().await {
            Ok(workspaces) => Response::Workspaces(workspaces),
            Err(failure) => Response::Failed(failure),
        },
        Request::Packs => match plane.packs().await {
            Ok(packs) => Response::Packs(packs),
            Err(failure) => Response::Failed(failure),
        },
        Request::ReviewPack { pack } => match plane.review_pack(&pack).await {
            Ok(review) => Response::PackReview(Box::new(review)),
            Err(failure) => Response::Failed(failure),
        },
        Request::InstallPack { pack, granting } => {
            match plane.install_pack(&pack, &granting).await {
                Ok(report) => Response::Edited(report),
                Err(failure) => Response::Failed(failure),
            }
        }
        Request::Reload => match plane.reload().await {
            Ok(status) => Response::Status(status),
            Err(failure) => Response::Failed(failure),
        },
    }
}

#[cfg(unix)]
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(SOCKET_MODE))
}

/// The control socket could not be opened.
#[derive(Debug, thiserror::Error)]
#[error("could not open the control socket at `{path}`")]
pub struct ServerError {
    /// The path that failed.
    pub path: PathBuf,
    /// What the operating system reported.
    #[source]
    pub source: std::io::Error,
}

impl ServerError {
    fn new(path: PathBuf, source: std::io::Error) -> Self {
        Self { path, source }
    }

    /// Whether another PushOS already holds this socket.
    ///
    /// Its own question because the answer is different in kind: every other
    /// failure means the socket is unavailable and PushOS runs without Studio,
    /// while this one means PushOS should not start at all.
    pub fn is_already_running(&self) -> bool {
        self.source.kind() == std::io::ErrorKind::AddrInUse
    }
}

/// Whether something is listening on a socket that exists.
///
/// Connecting is the only way to tell a live socket from one a killed process
/// left behind. A refused connection means nobody is home.
fn is_listening(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_over_long_path_is_refused_with_an_explanation() {
        let long = PathBuf::from(format!("/tmp/{}/control.sock", "d".repeat(120)));
        let error = ControlServer::bind(long).expect_err("the path is too long");

        let message = error.source.to_string();
        assert!(message.contains("at most"), "got `{message}`");
        assert!(
            message.contains("XDG_DATA_HOME"),
            "the message should say what to do"
        );
    }

    #[test]
    fn the_limit_leaves_room_for_the_shorter_of_the_two_platform_limits() {
        // macOS allows 104 bytes including the terminator; Linux allows 108.
        const { assert!(MAX_SOCKET_PATH < 104) };
    }
}
