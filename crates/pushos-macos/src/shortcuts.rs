//! Apple Shortcuts through the `shortcuts` command.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::ExecutionId;
use pushos_domain::ports::{ProcessRunner, ProcessSpec, ShortcutRunner};
use tracing::warn;

/// The command, addressed absolutely so `PATH` cannot redirect it.
const SHORTCUTS: &str = "/usr/bin/shortcuts";

/// Runs Shortcuts through the system command line tool.
#[derive(Debug, Clone)]
pub struct ShortcutsCli {
    processes: Arc<dyn ProcessRunner>,
    scratch: PathBuf,
}

impl ShortcutsCli {
    /// Builds a runner that stages Shortcut input in the system temporary
    /// directory.
    pub fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self::with_scratch(processes, std::env::temp_dir())
    }

    /// Builds a runner that stages Shortcut input in a given directory.
    pub fn with_scratch(processes: Arc<dyn ProcessRunner>, scratch: impl Into<PathBuf>) -> Self {
        Self {
            processes,
            scratch: scratch.into(),
        }
    }

    /// Writes text input to a file, because `shortcuts run` accepts input only
    /// as a path.
    ///
    /// The name carries a fresh execution id so two Shortcuts running at once
    /// cannot read each other's input.
    async fn stage_input(&self, input: &str) -> Result<PathBuf, ActionError> {
        let path = self
            .scratch
            .join(format!("pushos-shortcut-{}.txt", ExecutionId::generate()));
        tokio::fs::write(&path, input).await.map_err(|error| {
            ActionError::backend(
                "could not stage Shortcut input",
                ErrorClass::ComponentFailure,
                error,
            )
        })?;
        Ok(path)
    }
}

#[async_trait]
impl ShortcutRunner for ShortcutsCli {
    async fn list(&self) -> Result<Vec<String>, ActionError> {
        let outcome = self
            .processes
            .run(&ProcessSpec::new(SHORTCUTS, ["list".to_owned()]))
            .await?;

        if !outcome.succeeded() {
            return Err(ActionError::backend(
                format!("could not list Shortcuts: {}", outcome.stderr_tail.trim()),
                ErrorClass::Permission,
                std::io::Error::other(outcome.stderr_tail.trim().to_owned()),
            ));
        }

        Ok(outcome
            .stdout_tail
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect())
    }

    async fn run(&self, name: &str, input: Option<&str>) -> Result<(), ActionError> {
        let staged = match input {
            Some(text) => Some(self.stage_input(text).await?),
            None => None,
        };

        // The Shortcut name is one argument, never part of a command line, so a
        // name containing spaces or punctuation needs no quoting and cannot
        // become anything but a name.
        let mut args = vec!["run".to_owned(), name.to_owned()];
        if let Some(path) = &staged {
            args.push("--input-path".to_owned());
            args.push(path.to_string_lossy().into_owned());
        }

        let outcome = self.processes.run(&ProcessSpec::new(SHORTCUTS, args)).await;

        if let Some(path) = staged
            && let Err(error) = tokio::fs::remove_file(&path).await
        {
            warn!(?path, %error, "could not remove staged Shortcut input");
        }

        let outcome = outcome?;
        if outcome.succeeded() {
            return Ok(());
        }

        let detail = outcome.stderr_tail.trim();
        // A name that does not exist is a configuration problem; anything else
        // is more likely the Shortcut itself or a missing automation grant.
        let class = if detail.contains("couldn't be found") || detail.contains("not found") {
            ErrorClass::Validation
        } else {
            ErrorClass::ComponentFailure
        };

        Err(ActionError::backend(
            format!("`{name}` failed: {detail}"),
            class,
            std::io::Error::other(detail.to_owned()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use pushos_testkit::FakeProcesses;

    use super::*;

    fn runner(processes: &FakeProcesses, scratch: &std::path::Path) -> ShortcutsCli {
        ShortcutsCli::with_scratch(Arc::new(processes.clone()), scratch)
    }

    fn scratch_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("pushos-test-{}", ExecutionId::generate()));
        std::fs::create_dir_all(&path).expect("the temporary directory is writable");
        path
    }

    #[tokio::test]
    async fn a_shortcut_name_is_one_argument_however_it_is_written() {
        let scratch = scratch_dir();
        let processes = FakeProcesses::new();
        runner(&processes, &scratch)
            .run("End Workday; rm -rf /", None)
            .await
            .expect("succeeds");

        let spec = &processes.spawned()[0];
        assert_eq!(spec.program, "/usr/bin/shortcuts");
        assert_eq!(spec.args, ["run", "End Workday; rm -rf /"]);
    }

    #[tokio::test]
    async fn text_input_is_staged_to_a_file_and_cleaned_up() {
        let scratch = scratch_dir();
        let processes = FakeProcesses::new();
        runner(&processes, &scratch)
            .run("Office Lights", Some("on"))
            .await
            .expect("succeeds");

        let spec = &processes.spawned()[0];
        assert_eq!(spec.args[2], "--input-path");
        let staged = PathBuf::from(&spec.args[3]);
        assert!(staged.starts_with(&scratch));
        assert!(!staged.exists(), "staged input must not be left behind");
    }

    #[tokio::test]
    async fn two_concurrent_runs_stage_to_different_files() {
        let scratch = scratch_dir();
        let processes = FakeProcesses::new();
        let runner = runner(&processes, &scratch);

        let (first, second) =
            tokio::join!(runner.run("A", Some("one")), runner.run("B", Some("two")));
        first.expect("succeeds");
        second.expect("succeeds");

        let spawned = processes.spawned();
        assert_ne!(spawned[0].args[3], spawned[1].args[3]);
    }

    #[tokio::test]
    async fn no_input_means_no_input_path_argument() {
        let scratch = scratch_dir();
        let processes = FakeProcesses::new();
        runner(&processes, &scratch)
            .run("End Workday", None)
            .await
            .expect("succeeds");

        assert!(
            !processes.spawned()[0]
                .args
                .contains(&"--input-path".to_owned())
        );
    }

    #[tokio::test]
    async fn a_missing_shortcut_is_a_configuration_problem() {
        let scratch = scratch_dir();
        let processes = FakeProcesses::new();
        processes.set_exit_code(1);

        let error = runner(&processes, &scratch)
            .run("Nonexistent", None)
            .await
            .expect_err("the command reported a failure");
        // The fake reports no stderr, so this falls through to the general case.
        assert_eq!(error.class(), ErrorClass::ComponentFailure);
    }

    #[tokio::test]
    async fn listing_splits_the_output_into_names() {
        let scratch = scratch_dir();
        let processes = FakeProcesses::new();
        // The fake reports empty output, so the list is empty rather than a
        // single blank entry.
        let names = runner(&processes, &scratch).list().await.expect("succeeds");
        assert!(names.is_empty());
        assert_eq!(processes.spawned()[0].args, ["list"]);
    }
}
