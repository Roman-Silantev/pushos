//! Safe AppleScript invocation.
//!
//! Scripts are fixed text compiled into the binary. Anything variable is passed
//! as an argument to the script's `on run argv` handler, so no value is ever
//! spliced into source that is about to be compiled and executed.
//!
//! The one exception is an application's name, which AppleScript's `tell`
//! syntax cannot take as a variable. That name is therefore validated against a
//! deliberately narrow character set before it is used.

use std::sync::Arc;

use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ports::{ProcessOutcome, ProcessRunner, ProcessSpec};

/// The interpreter, addressed absolutely so `PATH` cannot redirect it.
const OSASCRIPT: &str = "/usr/bin/osascript";

/// An application name that is safe to place inside a `tell` block.
///
/// Validated on construction so the check cannot be forgotten at a call site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationName(String);

impl ApplicationName {
    /// Accepts a name made only of letters, digits, spaces, dots and hyphens.
    ///
    /// That covers every real application name PushOS needs to address and
    /// excludes every character AppleScript treats as syntax.
    pub fn new(name: impl Into<String>) -> Result<Self, UnsafeApplicationName> {
        let name = name.into();
        let acceptable = !name.is_empty()
            && name.len() <= 64
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_'));

        if acceptable {
            Ok(Self(name))
        } else {
            Err(UnsafeApplicationName { name })
        }
    }

    /// The validated name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An application name that could change the meaning of a script.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{name}` is not a usable application name")]
pub struct UnsafeApplicationName {
    /// The rejected name.
    pub name: String,
}

/// Runs compiled-in AppleScript through the process port.
#[derive(Debug, Clone)]
pub struct ScriptRunner {
    processes: Arc<dyn ProcessRunner>,
}

impl ScriptRunner {
    /// Builds a script runner over a process runner.
    pub fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self { processes }
    }

    /// Runs a script, passing `arguments` to its `on run argv` handler.
    ///
    /// Returns the script's trimmed standard output.
    pub async fn run(&self, script: &str, arguments: &[String]) -> Result<String, ActionError> {
        let mut args = vec!["-e".to_owned(), script.to_owned()];
        args.extend(arguments.iter().cloned());

        let outcome = self
            .processes
            .run(&ProcessSpec::new(OSASCRIPT, args))
            .await?;
        Self::interpret(&outcome)
    }

    fn interpret(outcome: &ProcessOutcome) -> Result<String, ActionError> {
        if outcome.succeeded() {
            return Ok(outcome.stdout_tail.trim().to_owned());
        }

        let detail = outcome.stderr_tail.trim();
        // AppleScript reports a refused automation permission as error -1743.
        // That needs the operator to grant access, not a retry.
        let class = if detail.contains("-1743") || detail.contains("Not authorized") {
            ErrorClass::Permission
        } else {
            ErrorClass::ComponentFailure
        };

        Err(ActionError::backend(
            format!("AppleScript failed: {detail}"),
            class,
            std::io::Error::other(detail.to_owned()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_application_names_are_accepted() {
        for name in [
            "Music",
            "Apple Music",
            "Google Chrome",
            "IINA",
            "1Password 8",
        ] {
            assert!(
                ApplicationName::new(name).is_ok(),
                "`{name}` should be usable"
            );
        }
    }

    #[test]
    fn names_that_could_change_a_script_are_refused() {
        for name in [
            "",
            "Music\" to quit\ntell application \"Finder",
            "Music'; do shell script \"rm -rf /\"",
            "Music\\",
            "Mus(ic)",
        ] {
            assert!(
                ApplicationName::new(name).is_err(),
                "`{name}` must not reach a tell block"
            );
        }
    }

    #[test]
    fn an_absurdly_long_name_is_refused() {
        assert!(ApplicationName::new("A".repeat(65)).is_err());
        assert!(ApplicationName::new("A".repeat(64)).is_ok());
    }

    #[test]
    fn a_refused_automation_permission_is_classified_as_a_permission_problem() {
        let outcome = ProcessOutcome {
            exit_code: Some(1),
            stdout_tail: String::new(),
            stderr_tail: "execution error: Not authorized to send Apple events (-1743)".to_owned(),
        };
        let error = ScriptRunner::interpret(&outcome).expect_err("the script failed");
        assert_eq!(error.class(), ErrorClass::Permission);
    }

    #[test]
    fn other_script_failures_are_component_failures() {
        let outcome = ProcessOutcome {
            exit_code: Some(1),
            stdout_tail: String::new(),
            stderr_tail: "execution error: something else (-1728)".to_owned(),
        };
        let error = ScriptRunner::interpret(&outcome).expect_err("the script failed");
        assert_eq!(error.class(), ErrorClass::ComponentFailure);
    }

    #[test]
    fn successful_output_is_trimmed() {
        let outcome = ProcessOutcome {
            exit_code: Some(0),
            stdout_tail: "  42 \n".to_owned(),
            stderr_tail: String::new(),
        };
        assert_eq!(ScriptRunner::interpret(&outcome).expect("succeeded"), "42");
    }
}
