//! What a running session can be asked to do.
//!
//! Answering a permission question is deliberately not here. The agent is
//! already parked on that question inside the session task, and handing the
//! answer straight to it is what unblocks the process; routing it through the
//! command queue would put it behind whatever else is waiting.

use tokio::sync::oneshot;

/// One instruction for a session task.
///
/// Every command carries a reply channel, so a caller learns whether the
/// session actually accepted it rather than assuming.
#[derive(Debug)]
pub(crate) enum SessionCommand {
    /// Send the operator's words.
    Prompt {
        /// What to say.
        text: String,
        /// Whether the session took it.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Interrupt whatever is being done.
    Cancel {
        /// Whether the interruption was sent.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// End the session and let the process go.
    Stop {
        /// Acknowledged once the process is on its way out.
        reply: oneshot::Sender<Result<(), String>>,
    },
}

impl SessionCommand {
    /// Reports that the session is gone, so no caller is left waiting.
    pub(crate) fn refuse(self, reason: &str) {
        let sender = match self {
            Self::Prompt { reply, .. } | Self::Cancel { reply } | Self::Stop { reply } => reply,
        };
        let _ = sender.send(Err(reason.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refusing_a_command_tells_whoever_was_waiting() {
        let (reply, answer) = oneshot::channel();
        SessionCommand::Prompt {
            text: "hello".to_owned(),
            reply,
        }
        .refuse("the session is gone");

        assert_eq!(
            answer.await.expect("answered"),
            Err("the session is gone".to_owned())
        );
    }

    #[tokio::test]
    async fn every_command_shape_can_be_refused() {
        let (reply, cancel) = oneshot::channel();
        SessionCommand::Cancel { reply }.refuse("gone");
        assert!(cancel.await.expect("answered").is_err());

        let (reply, stop) = oneshot::channel();
        SessionCommand::Stop { reply }.refuse("gone");
        assert!(stop.await.expect("answered").is_err());
    }
}
