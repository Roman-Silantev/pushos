//! Launching and focusing through `open`.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ports::{ApplicationLauncher, ApplicationTarget, ProcessRunner, ProcessSpec};

/// The launcher, addressed absolutely so `PATH` cannot redirect it.
const OPEN: &str = "/usr/bin/open";

/// Opens applications, files and URLs through the system launcher.
#[derive(Debug, Clone)]
pub struct OpenLauncher {
    processes: Arc<dyn ProcessRunner>,
}

impl OpenLauncher {
    /// Builds a launcher over a process runner.
    pub fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self { processes }
    }

    /// Builds the arguments for a target.
    ///
    /// Every value is a separate argument and `--` ends option parsing, so a
    /// path or URL beginning with a hyphen is opened rather than misread as a
    /// flag.
    fn arguments(target: &ApplicationTarget) -> Result<Vec<String>, ActionError> {
        Ok(match target {
            ApplicationTarget::Application { name } => {
                vec!["-a".to_owned(), name.clone()]
            }
            ApplicationTarget::Path { path, with } => {
                let path = path
                    .to_str()
                    .ok_or_else(|| ActionError::NeedsConfirmation {
                        reason: "the path is not valid text and cannot be opened".to_owned(),
                    })?
                    .to_owned();
                match with {
                    Some(application) => {
                        vec!["-a".to_owned(), application.clone(), "--".to_owned(), path]
                    }
                    None => vec!["--".to_owned(), path],
                }
            }
            ApplicationTarget::Url { url } => {
                Self::require_web_url(url)?;
                vec!["--".to_owned(), url.clone()]
            }
        })
    }

    /// Refuses URL schemes that would hand arbitrary execution to the system.
    ///
    /// `open` will happily launch a registered handler for any scheme, so a
    /// configured `file://` or custom scheme is a much broader capability than
    /// "open a link". PushOS grants only the web schemes here; anything else
    /// belongs behind an explicit, separately permissioned action.
    fn require_web_url(url: &str) -> Result<(), ActionError> {
        if url.starts_with("https://") || url.starts_with("http://") {
            Ok(())
        } else {
            Err(ActionError::NeedsConfirmation {
                reason: format!("`{url}` is not an http or https address"),
            })
        }
    }
}

#[async_trait]
impl ApplicationLauncher for OpenLauncher {
    async fn open(&self, target: &ApplicationTarget) -> Result<(), ActionError> {
        let outcome = self
            .processes
            .run(&ProcessSpec::new(OPEN, Self::arguments(target)?))
            .await?;

        if outcome.succeeded() {
            return Ok(());
        }

        Err(ActionError::backend(
            format!("could not open the target: {}", outcome.stderr_tail.trim()),
            ErrorClass::Validation,
            std::io::Error::other(outcome.stderr_tail.trim().to_owned()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pushos_testkit::FakeProcesses;

    use super::*;

    fn launcher(processes: &FakeProcesses) -> OpenLauncher {
        OpenLauncher::new(Arc::new(processes.clone()))
    }

    #[tokio::test]
    async fn an_application_is_opened_by_name() {
        let processes = FakeProcesses::new();
        launcher(&processes)
            .open(&ApplicationTarget::Application {
                name: "Cursor".to_owned(),
            })
            .await
            .expect("succeeds");

        let spec = &processes.spawned()[0];
        assert_eq!(spec.program, "/usr/bin/open");
        assert_eq!(spec.args, ["-a", "Cursor"]);
    }

    #[tokio::test]
    async fn a_path_beginning_with_a_hyphen_is_opened_rather_than_read_as_a_flag() {
        let processes = FakeProcesses::new();
        launcher(&processes)
            .open(&ApplicationTarget::Path {
                path: PathBuf::from("-rf"),
                with: None,
            })
            .await
            .expect("succeeds");

        assert_eq!(processes.spawned()[0].args, ["--", "-rf"]);
    }

    #[tokio::test]
    async fn a_path_may_name_the_application_that_opens_it() {
        let processes = FakeProcesses::new();
        launcher(&processes)
            .open(&ApplicationTarget::Path {
                path: PathBuf::from("/tmp/project"),
                with: Some("Cursor".to_owned()),
            })
            .await
            .expect("succeeds");

        assert_eq!(
            processes.spawned()[0].args,
            ["-a", "Cursor", "--", "/tmp/project"]
        );
    }

    #[tokio::test]
    async fn only_web_addresses_may_be_opened_as_urls() {
        let processes = FakeProcesses::new();
        for url in ["https://example.com", "http://example.com"] {
            launcher(&processes)
                .open(&ApplicationTarget::Url {
                    url: url.to_owned(),
                })
                .await
                .expect("web addresses are allowed");
        }
        assert_eq!(processes.spawned().len(), 2);
    }

    #[tokio::test]
    async fn other_url_schemes_are_refused_rather_than_handed_to_the_system() {
        let processes = FakeProcesses::new();
        for url in [
            "file:///etc/passwd",
            "x-apple-shortcut://run?name=Wipe",
            "javascript:alert(1)",
            "ssh://root@example.com",
        ] {
            let error = launcher(&processes)
                .open(&ApplicationTarget::Url {
                    url: url.to_owned(),
                })
                .await
                .expect_err("only web addresses are allowed");
            assert_eq!(error.class(), ErrorClass::UserActionRequired);
        }
        assert!(
            processes.spawned().is_empty(),
            "nothing may be handed to a URL handler"
        );
    }

    #[tokio::test]
    async fn a_failure_from_open_is_reported_as_a_validation_problem() {
        let processes = FakeProcesses::new();
        processes.set_exit_code(1);

        let error = launcher(&processes)
            .open(&ApplicationTarget::Application {
                name: "Nonexistent".to_owned(),
            })
            .await
            .expect_err("open reported a failure");
        assert_eq!(error.class(), ErrorClass::Validation);
    }
}
