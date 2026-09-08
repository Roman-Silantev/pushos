//! Driving terminals from the surface.
//!
//! The provider translates a gesture into one supervisor call and nothing more.
//! It holds no terminal state and starts nothing the operator did not ask for.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionResult, ActionStatus, DisplayIntent};
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::{ActionVerb, ProviderName, WorkspaceId};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::{ActionProvider, ProviderCapabilities, TerminalStatus};
use pushos_domain::terminal::TerminalTarget;
use pushos_terminal::{OpenTerminal, TerminalSummary, TerminalSupervisor};
use tracing::info;

/// The namespace this provider claims.
pub const NAMESPACE: &str = "terminal";

/// Turns gestures into terminal operations.
#[derive(Debug)]
pub struct TerminalProvider {
    supervisor: Arc<TerminalSupervisor>,
}

impl TerminalProvider {
    /// Builds the provider over a supervisor.
    pub fn new(supervisor: Arc<TerminalSupervisor>) -> Self {
        Self { supervisor }
    }

    /// Reads the target a binding names.
    ///
    /// An absent target means the selected terminal, which is what makes a
    /// single "stop" pad useful across every terminal on the surface.
    fn target(context: &ActionContext) -> Result<TerminalTarget, ActionError> {
        let params = context.params();
        // `terminal.open` names what it opens, and acting on that same name
        // afterwards is the common case, so a bare name counts as a target.
        let written = match (params.text("target"), params.text("name")) {
            (Some(target), _) => target.to_owned(),
            (None, Some(name)) => format!("name:{name}"),
            (None, None) => "selected".to_owned(),
        };

        written
            .parse()
            .map_err(|error: pushos_domain::terminal::MalformedTerminalTarget| {
                ActionError::backend(error.to_string(), ErrorClass::Validation, error)
            })
    }

    /// Reads what to open.
    fn request(context: &ActionContext) -> Result<OpenTerminal, ActionError> {
        let params = context.params();
        let name = params.require_text("name")?;
        if name.trim().is_empty() {
            return Err(invalid("a terminal needs a name"));
        }

        Ok(OpenTerminal {
            name: name.trim().to_owned(),
            program: params.text("program").map(ToOwned::to_owned),
            args: params
                .text_list("args")
                .unwrap_or_default()
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
            cwd: params.text("cwd").map(Into::into),
            workspace: params.text("workspace").map(WorkspaceId::new),
        })
    }

    /// The command line to run.
    ///
    /// A blank line submits an empty command, which does nothing and looks like
    /// the binding worked, so it is refused instead.
    fn line(context: &ActionContext) -> Result<&str, ActionError> {
        let text = context.params().require_text("text")?;
        if text.trim().is_empty() {
            return Err(invalid("there is nothing to run"));
        }
        Ok(text)
    }

    /// The keystrokes to type, exactly as written.
    ///
    /// Whitespace is meaningful here: a lone return answers a prompt, and a
    /// space chooses in a pager. Only nothing at all is refused.
    fn keys(context: &ActionContext) -> Result<&str, ActionError> {
        let text = context.params().require_text("text")?;
        if text.is_empty() {
            return Err(invalid("there is nothing to type"));
        }
        Ok(text)
    }

    /// Reports what a terminal is now doing.
    async fn describe(
        &self,
        target: &TerminalTarget,
        verb: &str,
    ) -> Result<ActionResult, ActionError> {
        let summary = self.summary(target).await;
        Ok(summary.map_or_else(ActionResult::completed, |summary| report(&summary, verb)))
    }

    /// Finds a terminal after acting on it, which may have finished meanwhile.
    async fn summary(&self, target: &TerminalTarget) -> Option<TerminalSummary> {
        let listed = self.supervisor.summaries().await;
        let wanted = match target {
            TerminalTarget::Session(id) => return listed.into_iter().find(|s| s.id == *id),
            TerminalTarget::Selected => return listed.into_iter().find(|s| s.selected),
            TerminalTarget::Named(name) => name.clone(),
        };
        // The most recently opened terminal holding the name, matching how the
        // registry resolves one.
        listed.into_iter().rfind(|summary| summary.name == wanted)
    }
}

#[async_trait]
impl ActionProvider for TerminalProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(["open", "run", "send", "select", "close"].map(ActionVerb::new))
            // A terminal runs whatever is typed into it, so the namespace as a
            // whole is gated rather than each verb separately.
            .requiring([Permission::ShellExecute])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        let verb = context.definition.selector.verb.to_string();

        match verb.as_str() {
            "open" => {
                let request = Self::request(&context)?;
                info!(name = %request.name, "opening a terminal");
                let summary = self
                    .supervisor
                    .open(request)
                    .await
                    .map_err(into_action_error)?;
                Ok(report(&summary, &verb))
            }
            "run" => {
                let target = Self::target(&context)?;
                let line = Self::line(&context)?;
                info!(%target, "running a command in a terminal");
                self.supervisor
                    .run(&target, line)
                    .await
                    .map_err(into_action_error)?;
                self.describe(&target, &verb).await
            }
            "send" => {
                let target = Self::target(&context)?;
                let text = Self::keys(&context)?;
                self.supervisor
                    .write(&target, text)
                    .await
                    .map_err(into_action_error)?;
                self.describe(&target, &verb).await
            }
            "select" => {
                let target = Self::target(&context)?;
                self.supervisor
                    .select(&target)
                    .await
                    .map_err(into_action_error)?;
                self.describe(&target, &verb).await
            }
            "close" => {
                let target = Self::target(&context)?;
                info!(%target, "ending a terminal");
                self.supervisor
                    .close(&target)
                    .await
                    .map_err(into_action_error)?;

                // Reported without re-reading the terminal: the program has
                // been asked to stop but has not necessarily gone yet, and
                // "starting" is the last thing an operator should be told
                // after pressing stop.
                Ok(ActionResult {
                    status: ActionStatus::Completed,
                    message: Some(format!("{target} stopping")),
                    display: None,
                })
            }
            other => Err(ActionError::UnknownVerb {
                provider: self.name(),
                verb: other.to_owned(),
            }),
        }
    }
}

/// Turns the outcome into something the surface can show.
fn report(summary: &TerminalSummary, verb: &str) -> ActionResult {
    let status = match summary.status {
        TerminalStatus::Starting | TerminalStatus::Running => ActionStatus::Started,
        TerminalStatus::Exited { code: Some(0) } | TerminalStatus::Stopped => {
            ActionStatus::Completed
        }
        TerminalStatus::Exited { .. } | TerminalStatus::Failed => ActionStatus::Failed,
    };

    // Opening earns a banner because nothing appears on screen to say it
    // happened. Everything else gets a line, so a running build is not
    // interrupted by news about itself.
    let display = (verb == "open").then(|| DisplayIntent::Toast {
        title: summary.name.clone(),
        detail: Some(summary.program.clone()),
    });

    let message = match summary.last_line.as_deref() {
        Some(said) => format!("{}: {said}", summary.name),
        None => format!("{} {}", summary.name, describe_status(summary.status)),
    };

    ActionResult {
        status,
        message: Some(message),
        display,
    }
}

/// How a status reads on the display.
const fn describe_status(status: TerminalStatus) -> &'static str {
    match status {
        TerminalStatus::Starting => "starting",
        TerminalStatus::Running => "running",
        TerminalStatus::Exited { code: Some(0) } => "finished",
        TerminalStatus::Stopped => "stopped",
        TerminalStatus::Exited { .. } => "failed",
        TerminalStatus::Failed => "did not start",
    }
}

fn invalid(reason: &'static str) -> ActionError {
    ActionError::backend(
        reason,
        ErrorClass::Validation,
        std::io::Error::other(reason),
    )
}

/// Keeps a terminal failure's classification as it crosses into the action layer.
fn into_action_error(error: pushos_domain::ports::TerminalError) -> ActionError {
    let class = error.class();
    ActionError::backend(error.to_string(), class, error)
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::ids::CorrelationId;
    use pushos_testkit::{FakeTerminal, RecordingTerminalObserver};

    use super::*;

    struct Fixture {
        provider: TerminalProvider,
        supervisor: Arc<TerminalSupervisor>,
        host: FakeTerminal,
        observer: RecordingTerminalObserver,
    }

    impl Fixture {
        fn new() -> Self {
            let observer = RecordingTerminalObserver::new();
            let host = FakeTerminal::new(Arc::new(observer.clone()));
            let supervisor = Arc::new(
                TerminalSupervisor::new(
                    Arc::new(host.clone()),
                    Arc::new(pushos_testkit::FixedRoot::new("/tmp")),
                )
                .with_shell("/bin/zsh"),
            );
            Self {
                provider: TerminalProvider::new(Arc::clone(&supervisor)),
                supervisor,
                host,
                observer,
            }
        }

        async fn run(&self, verb: &str, params: Params) -> Result<ActionResult, ActionError> {
            let definition = ActionDefinition::new(ActionSelector::new(NAMESPACE, verb), params);
            self.provider
                .execute(ActionContext::new(
                    definition,
                    CorrelationId::generate(),
                    SurfaceContext::empty(),
                ))
                .await
        }

        async fn drain(&self) {
            for (id, event) in self.observer.events() {
                self.supervisor.apply(&id, &event).await;
            }
        }
    }

    fn params(pairs: &[(&str, &str)]) -> Params {
        let mut params = Params::new();
        for (key, value) in pairs {
            params.set(*key, ParamValue::Text((*value).into()));
        }
        params
    }

    #[tokio::test]
    async fn opening_a_terminal_reports_it_and_says_so_on_the_display() {
        // Nothing appears on screen when a managed terminal starts, so the
        // action has to be what tells the operator it worked.
        let fixture = Fixture::new();
        let result = fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");

        assert_eq!(result.status, ActionStatus::Started);
        assert!(matches!(
            result.display,
            Some(DisplayIntent::Toast { ref title, .. }) if title == "tests"
        ));
        assert_eq!(fixture.host.open_count(), 1);
    }

    #[tokio::test]
    async fn opening_passes_the_program_and_arguments_through() {
        let fixture = Fixture::new();
        let mut requested = params(&[("name", "tests"), ("program", "cargo"), ("cwd", "/usr")]);
        requested.set(
            "args",
            ParamValue::List(vec![
                ParamValue::Text("test".into()),
                ParamValue::Text("--all".into()),
            ]),
        );

        fixture
            .run("open", requested)
            .await
            .expect("the fake succeeds by default");

        let listed = fixture.supervisor.summaries().await;
        let spec = fixture.host.spec(&listed[0].id).expect("it was opened");
        assert_eq!(spec.program, "cargo");
        assert_eq!(spec.args, ["test", "--all"]);
        assert_eq!(spec.cwd, std::path::PathBuf::from("/usr"));
    }

    #[tokio::test]
    async fn opening_without_a_name_is_refused() {
        let fixture = Fixture::new();
        let error = fixture
            .run("open", Params::new())
            .await
            .expect_err("a terminal needs a name");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn running_a_command_types_it_and_presses_return() {
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");

        fixture
            .run("run", params(&[("name", "tests"), ("text", "cargo test")]))
            .await
            .expect("it is running");

        let listed = fixture.supervisor.summaries().await;
        assert_eq!(
            fixture.host.typed(&listed[0].id).as_deref(),
            Some("cargo test\r")
        );
    }

    #[tokio::test]
    async fn sending_text_types_it_exactly() {
        // Answering a prompt with a single character must not submit a line.
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");

        fixture
            .run("send", params(&[("name", "tests"), ("text", "y")]))
            .await
            .expect("it is running");

        let listed = fixture.supervisor.summaries().await;
        assert_eq!(fixture.host.typed(&listed[0].id).as_deref(), Some("y"));
    }

    #[tokio::test]
    async fn an_action_with_no_target_acts_on_the_selected_terminal() {
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "build")]))
            .await
            .expect("the fake succeeds by default");
        fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");

        fixture
            .run("run", params(&[("text", "ls")]))
            .await
            .expect("something is selected");

        let listed = fixture.supervisor.summaries().await;
        let selected = listed.iter().find(|s| s.selected).expect("one is selected");
        assert_eq!(selected.name, "tests");
        assert_eq!(fixture.host.typed(&selected.id).as_deref(), Some("ls\r"));
    }

    #[tokio::test]
    async fn an_exact_session_target_is_honoured() {
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "build")]))
            .await
            .expect("the fake succeeds by default");
        let build = fixture.supervisor.summaries().await[0].id.clone();
        fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");

        fixture
            .run(
                "run",
                params(&[("target", &format!("session:{build}")), ("text", "make")]),
            )
            .await
            .expect("it exists");
        assert_eq!(fixture.host.typed(&build).as_deref(), Some("make\r"));
    }

    #[tokio::test]
    async fn selecting_moves_where_unqualified_actions_go() {
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "build")]))
            .await
            .expect("the fake succeeds by default");
        fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");

        fixture
            .run("select", params(&[("name", "build")]))
            .await
            .expect("it exists");

        let listed = fixture.supervisor.summaries().await;
        let selected = listed.iter().find(|s| s.selected).expect("one is selected");
        assert_eq!(selected.name, "build");
    }

    #[tokio::test]
    async fn closing_ends_the_terminal() {
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");

        fixture
            .run("close", params(&[("name", "tests")]))
            .await
            .expect("it is running");
        fixture.drain().await;

        assert_eq!(fixture.supervisor.live_count().await, 0);
    }

    #[tokio::test]
    async fn stopping_says_so_rather_than_reporting_the_terminal_as_starting() {
        // The program has been asked to stop but has not gone yet, and
        // "starting" is the last thing to tell an operator who pressed stop.
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "server")]))
            .await
            .expect("the fake succeeds by default");

        let result = fixture
            .run("close", params(&[("name", "server")]))
            .await
            .expect("it is running");
        assert_eq!(result.status, ActionStatus::Completed);
        assert_eq!(result.message.as_deref(), Some("name:server stopping"));
    }

    #[tokio::test]
    async fn the_result_carries_the_last_thing_the_terminal_said() {
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");
        let id = fixture.supervisor.summaries().await[0].id.clone();
        fixture.host.emit(&id, "77 passed\r\n");
        fixture.drain().await;

        let result = fixture
            .run("send", params(&[("name", "tests"), ("text", "\n")]))
            .await
            .expect("it is running");
        assert_eq!(result.message.as_deref(), Some("tests: 77 passed"));
    }

    #[tokio::test]
    async fn a_terminal_that_failed_is_reported_as_a_failure() {
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");
        let id = fixture.supervisor.summaries().await[0].id.clone();
        fixture.host.finish(&id, Some(1));
        fixture.drain().await;

        let error = fixture
            .run("run", params(&[("name", "tests"), ("text", "again")]))
            .await
            .expect_err("it has finished");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn running_a_blank_line_is_refused_rather_than_reported_as_done() {
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");

        let error = fixture
            .run("run", params(&[("name", "tests"), ("text", "   ")]))
            .await
            .expect_err("there is nothing to run");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn sending_a_lone_return_is_allowed_because_it_answers_a_prompt() {
        // `send` is keystrokes, not a command line, so whitespace counts.
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");

        fixture
            .run("send", params(&[("name", "tests"), ("text", "\r")]))
            .await
            .expect("a return is something to type");

        let listed = fixture.supervisor.summaries().await;
        assert_eq!(fixture.host.typed(&listed[0].id).as_deref(), Some("\r"));
    }

    #[tokio::test]
    async fn sending_nothing_at_all_is_still_refused() {
        let fixture = Fixture::new();
        fixture
            .run("open", params(&[("name", "tests")]))
            .await
            .expect("the fake succeeds by default");

        let error = fixture
            .run("send", params(&[("name", "tests"), ("text", "")]))
            .await
            .expect_err("there is nothing to type");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn a_malformed_target_says_what_one_looks_like() {
        let fixture = Fixture::new();
        let error = fixture
            .run("run", params(&[("target", "role:builder"), ("text", "x")]))
            .await
            .expect_err("that is an agent target");
        assert!(error.to_string().contains("name:tests"), "{error}");
    }

    #[tokio::test]
    async fn a_verb_the_provider_does_not_have_is_refused() {
        let fixture = Fixture::new();
        let error = fixture
            .run("detonate", params(&[("name", "tests")]))
            .await
            .expect_err("there is no such verb");
        assert!(matches!(error, ActionError::UnknownVerb { .. }));
    }

    #[test]
    fn every_verb_the_provider_answers_is_one_it_declares() {
        let observer = RecordingTerminalObserver::new();
        let host = FakeTerminal::new(Arc::new(observer));
        let supervisor = Arc::new(TerminalSupervisor::new(
            Arc::new(host),
            Arc::new(pushos_testkit::FixedRoot::new("/tmp")),
        ));
        let capabilities = TerminalProvider::new(supervisor).capabilities();

        for verb in ["open", "run", "send", "select", "close"] {
            assert!(
                capabilities.accepts(&ActionVerb::new(verb)),
                "`{verb}` is answered but not declared"
            );
        }
        assert!(
            capabilities
                .required_permissions()
                .contains(&Permission::ShellExecute)
        );
    }
}
