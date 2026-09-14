//! Running subprocesses on the host.
//!
//! Arguments are handed to the kernel as a vector, never assembled into a
//! command line, so nothing configured or transcribed can become shell syntax.

use std::process::Stdio;

use async_trait::async_trait;
use pushos_actions::providers::shell::classify_spawn_failure;
use pushos_domain::error::ActionError;
use pushos_domain::ports::{ProcessOutcome, ProcessRunner, ProcessSpec};
use tokio::process::Command;
use tracing::debug;

/// Runs programs through the operating system.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProcessRunner;

impl SystemProcessRunner {
    /// Builds a runner.
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ProcessRunner for SystemProcessRunner {
    async fn run(&self, spec: &ProcessSpec) -> Result<ProcessOutcome, ActionError> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Without this, a process outliving PushOS would keep running
            // unsupervised after the runtime that started it is gone.
            .kill_on_drop(true);

        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }

        debug!(program = %spec.program, args = ?spec.args, "running process");

        let child = command.spawn().map_err(|error| {
            let class = classify_spawn_failure(&error);
            ActionError::backend(format!("could not start `{}`", spec.program), class, error)
        })?;

        let output = match tokio::time::timeout(spec.timeout, child.wait_with_output()).await {
            Ok(result) => result.map_err(|error| {
                ActionError::backend(
                    format!("`{}` could not be waited on", spec.program),
                    classify_spawn_failure(&error),
                    error,
                )
            })?,
            Err(_) => {
                // `kill_on_drop` ends the child as this future unwinds.
                return Err(ActionError::TimedOut {
                    selector: spec.program.clone(),
                    timeout_ms: u64::try_from(spec.timeout.as_millis()).unwrap_or(u64::MAX),
                });
            }
        };

        Ok(ProcessOutcome {
            exit_code: output.status.code(),
            stdout_tail: tail(&output.stdout, spec.capture),
            stderr_tail: tail(&output.stderr, spec.capture),
        })
    }
}

/// Keeps the end of a stream, which is where a failure's explanation is.
fn tail(bytes: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= limit {
        return text.into_owned();
    }
    let start = text
        .char_indices()
        .rev()
        .take_while(|(index, _)| text.len() - index <= limit)
        .last()
        .map_or(0, |(index, _)| index);
    text[start..].to_owned()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pushos_domain::error::ErrorClass;

    use super::*;

    #[tokio::test]
    async fn a_successful_program_reports_its_output() {
        let outcome = SystemProcessRunner::new()
            .run(&ProcessSpec::new("/bin/echo", ["hello".to_owned()]))
            .await
            .expect("echo is present on every macOS and Linux host");

        assert!(outcome.succeeded());
        assert_eq!(outcome.stdout_tail.trim(), "hello");
    }

    #[tokio::test]
    async fn arguments_are_never_interpreted_as_shell_syntax() {
        let outcome = SystemProcessRunner::new()
            .run(&ProcessSpec::new("/bin/echo", ["a && b; c".to_owned()]))
            .await
            .expect("echo runs");

        assert_eq!(
            outcome.stdout_tail.trim(),
            "a && b; c",
            "the argument reached the program verbatim rather than being run"
        );
    }

    #[tokio::test]
    async fn a_failing_program_reports_its_exit_code_rather_than_erroring() {
        let outcome = SystemProcessRunner::new()
            .run(&ProcessSpec::new(
                "/bin/sh",
                ["-c".to_owned(), "exit 3".to_owned()],
            ))
            .await
            .expect("the program ran, it simply failed");

        assert_eq!(outcome.exit_code, Some(3));
        assert!(!outcome.succeeded());
    }

    #[tokio::test]
    async fn a_missing_program_is_a_validation_failure() {
        let error = SystemProcessRunner::new()
            .run(&ProcessSpec::new("/definitely/not/here", []))
            .await
            .expect_err("there is no such program");

        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(!error.is_retryable());
    }

    #[tokio::test]
    async fn a_program_that_overruns_its_budget_is_killed() {
        let spec =
            ProcessSpec::new("/bin/sleep", ["60".to_owned()]).within(Duration::from_millis(100));

        let error = SystemProcessRunner::new()
            .run(&spec)
            .await
            .expect_err("the program outlives its budget");

        assert!(matches!(error, ActionError::TimedOut { .. }));
    }

    #[tokio::test]
    async fn the_working_directory_is_honoured() {
        let spec = ProcessSpec::new("/bin/pwd", []).in_directory("/tmp");
        let outcome = SystemProcessRunner::new()
            .run(&spec)
            .await
            .expect("pwd runs");
        assert!(outcome.stdout_tail.trim().ends_with("/tmp"));
    }

    #[test]
    fn capture_keeps_the_end_of_a_long_stream_on_a_character_boundary() {
        let limit = ProcessSpec::DEFAULT_CAPTURE;
        let long = "é".repeat(limit);
        let kept = tail(long.as_bytes(), limit);
        assert!(kept.len() <= limit);
        assert!(long.ends_with(&kept), "the tail, not the head, is kept");
    }

    #[tokio::test]
    async fn a_program_whose_output_is_the_answer_can_keep_all_of_it() {
        let spec = ProcessSpec::new(
            "/bin/sh",
            [
                "-c".to_owned(),
                "head -c 20000 /dev/zero | tr '\\0' x".to_owned(),
            ],
        )
        .capturing(64 * 1024);
        let outcome = SystemProcessRunner::new()
            .run(&spec)
            .await
            .expect("sh runs");
        assert_eq!(outcome.stdout_tail.len(), 20_000);
    }
}
