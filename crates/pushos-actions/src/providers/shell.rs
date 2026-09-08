//! Running a program.
//!
//! There is deliberately no way to express a shell command line here. A shell
//! action names a program and its arguments separately, and they stay separate
//! all the way to the syscall, so no configured or spoken value can ever be
//! reinterpreted as shell syntax.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionResult, ActionStatus};
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::{ActionVerb, ProviderName};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::{ActionProvider, ProcessRunner, ProcessSpec, ProviderCapabilities};

/// The namespace this provider claims.
pub const NAMESPACE: &str = "shell";

/// How much of a failing process's output is worth showing on a 960 by 160 display.
const MESSAGE_LIMIT: usize = 160;

/// Runs configured programs.
#[derive(Debug)]
pub struct ShellProvider {
    runner: Arc<dyn ProcessRunner>,
}

impl ShellProvider {
    /// Builds the provider over a process runner.
    pub fn new(runner: Arc<dyn ProcessRunner>) -> Self {
        Self { runner }
    }

    /// Builds a process specification from an action's parameters.
    fn spec(context: &ActionContext) -> Result<ProcessSpec, ActionError> {
        let params = context.params();
        let program = params.require_text("program")?;

        // A program name containing shell metacharacters is a sign that
        // someone expected a command line to be interpreted. It will not be,
        // so say so rather than failing later with a confusing "not found".
        if program.contains(|c: char| " \t\n|&;<>()$`\\\"'".contains(c)) {
            return Err(ActionError::NeedsConfirmation {
                reason: format!(
                    "`{program}` looks like a command line; \
                     give `program` and `args` separately"
                ),
            });
        }

        let args = params
            .text_list("args")
            .unwrap_or_default()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();

        let mut spec = ProcessSpec::new(program, args);
        if let Some(cwd) = params.text("cwd") {
            spec = spec.in_directory(cwd);
        }
        if let Some(seconds) = params
            .get("timeout_seconds")
            .and_then(pushos_domain::action::ParamValue::as_integer)
        {
            spec = spec.within(Duration::from_secs(seconds.clamp(1, 3_600).unsigned_abs()));
        }
        Ok(spec)
    }
}

#[async_trait]
impl ActionProvider for ShellProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new([ActionVerb::new("run")]).requiring([Permission::ShellExecute])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        if context.definition.selector.verb.as_str() != "run" {
            return Err(ActionError::UnknownVerb {
                provider: self.name(),
                verb: context.definition.selector.verb.to_string(),
            });
        }

        let spec = Self::spec(&context)?;
        let program = spec.program.clone();
        let outcome = self.runner.run(&spec).await?;

        if outcome.succeeded() {
            return Ok(ActionResult::completed().with_message(format!("{program} succeeded")));
        }

        // A non-zero exit is the program's answer, not a PushOS fault, so it is
        // reported as a failed action rather than raised as an error.
        let detail = summarise(&outcome.stderr_tail, &outcome.stdout_tail);
        Ok(ActionResult {
            status: ActionStatus::Failed,
            message: Some(match outcome.exit_code {
                Some(code) => format!("{program} exited {code}: {detail}"),
                None => format!("{program} was terminated: {detail}"),
            }),
            display: None,
        })
    }
}

/// Picks the more informative of a process's two output streams, and trims it.
fn summarise(stderr: &str, stdout: &str) -> String {
    let source = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let trimmed = source.trim();
    if trimmed.len() <= MESSAGE_LIMIT {
        return trimmed.to_owned();
    }
    let cut = trimmed
        .char_indices()
        .take_while(|(index, _)| *index <= MESSAGE_LIMIT)
        .last()
        .map_or(0, |(index, _)| index);
    format!("{}...", &trimmed[..cut])
}

/// Classifies a failure to even start a process.
///
/// Exposed so that host adapters classify consistently rather than each
/// inventing their own mapping.
pub fn classify_spawn_failure(error: &std::io::Error) -> ErrorClass {
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => ErrorClass::Validation,
        std::io::ErrorKind::PermissionDenied => ErrorClass::Permission,
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted => ErrorClass::Retryable,
        _ => ErrorClass::ComponentFailure,
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::ids::CorrelationId;
    use pushos_testkit::FakeProcesses;

    use super::*;

    async fn run(processes: &FakeProcesses, params: Params) -> Result<ActionResult, ActionError> {
        let definition = ActionDefinition::new(ActionSelector::new(NAMESPACE, "run"), params);
        ShellProvider::new(Arc::new(processes.clone()))
            .execute(ActionContext::new(
                definition,
                CorrelationId::generate(),
                SurfaceContext::empty(),
            ))
            .await
    }

    fn cargo_test() -> Params {
        let mut params = Params::new();
        params.set("program", ParamValue::from("cargo"));
        params.set(
            "args",
            ParamValue::List(vec![ParamValue::from("test"), ParamValue::from("--all")]),
        );
        params
    }

    #[tokio::test]
    async fn a_program_and_its_arguments_stay_separate() {
        let processes = FakeProcesses::new();
        run(&processes, cargo_test()).await.expect("succeeds");

        let spawned = processes.spawned();
        assert_eq!(spawned[0].program, "cargo");
        assert_eq!(spawned[0].args, ["test", "--all"]);
    }

    #[tokio::test]
    async fn a_command_line_in_the_program_field_is_refused_rather_than_interpreted() {
        let processes = FakeProcesses::new();
        let mut params = Params::new();
        params.set("program", ParamValue::from("cargo test && rm -rf /"));

        let error = run(&processes, params)
            .await
            .expect_err("this is not a command line");
        assert_eq!(error.class(), ErrorClass::UserActionRequired);
        assert!(
            processes.spawned().is_empty(),
            "nothing may run on a misread specification"
        );
    }

    #[tokio::test]
    async fn arguments_containing_shell_syntax_are_passed_through_untouched() {
        // Arguments are never interpreted, so they need no escaping and no
        // rejection. This is the whole reason the two fields are separate.
        let processes = FakeProcesses::new();
        let mut params = Params::new();
        params.set("program", ParamValue::from("echo"));
        params.set("args", ParamValue::from("; rm -rf / #"));

        run(&processes, params).await.expect("succeeds");
        assert_eq!(processes.spawned()[0].args, ["; rm -rf / #"]);
    }

    #[tokio::test]
    async fn a_non_zero_exit_is_a_failed_action_not_an_error() {
        let processes = FakeProcesses::new();
        processes.set_exit_code(101);

        let result = run(&processes, cargo_test())
            .await
            .expect("the process ran");
        assert_eq!(result.status, ActionStatus::Failed);
        assert!(result.message.unwrap_or_default().contains("101"));
    }

    #[tokio::test]
    async fn a_timeout_is_clamped_to_something_survivable() {
        let processes = FakeProcesses::new();
        let mut params = cargo_test();
        params.set("timeout_seconds", ParamValue::Integer(999_999));
        run(&processes, params).await.expect("succeeds");

        assert_eq!(processes.spawned()[0].timeout, Duration::from_secs(3_600));
    }

    #[tokio::test]
    async fn a_working_directory_is_passed_through_when_given() {
        let processes = FakeProcesses::new();
        let mut params = cargo_test();
        params.set("cwd", ParamValue::from("/tmp/project"));
        run(&processes, params).await.expect("succeeds");

        assert_eq!(
            processes.spawned()[0].cwd.as_deref(),
            Some(std::path::Path::new("/tmp/project"))
        );
    }

    #[tokio::test]
    async fn running_without_a_program_is_a_validation_error() {
        let processes = FakeProcesses::new();
        let error = run(&processes, Params::new())
            .await
            .expect_err("no program");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[test]
    fn shell_execution_is_a_declared_capability() {
        let provider = ShellProvider::new(Arc::new(FakeProcesses::new()));
        assert_eq!(
            provider.capabilities().required_permissions(),
            [Permission::ShellExecute]
        );
    }

    #[test]
    fn output_summaries_prefer_the_error_stream_and_stay_short() {
        assert_eq!(summarise("boom", "fine"), "boom");
        assert_eq!(summarise("   ", "fallback"), "fallback");

        let long = "x".repeat(500);
        let summary = summarise(&long, "");
        assert!(summary.len() <= MESSAGE_LIMIT + 4);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn a_summary_never_splits_a_character_in_half() {
        let long = "é".repeat(500);
        let summary = summarise(&long, "");
        assert!(summary.ends_with("..."));
        assert!(summary.is_char_boundary(summary.len() - 3));
    }

    #[test]
    fn spawn_failures_are_classified_by_cause() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            classify_spawn_failure(&Error::new(ErrorKind::NotFound, "x")),
            ErrorClass::Validation
        );
        assert_eq!(
            classify_spawn_failure(&Error::new(ErrorKind::PermissionDenied, "x")),
            ErrorClass::Permission
        );
        assert_eq!(
            classify_spawn_failure(&Error::other("x")),
            ErrorClass::ComponentFailure
        );
    }
}
