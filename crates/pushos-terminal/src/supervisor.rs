//! The single owner of every terminal.
//!
//! Everything that starts, types into, selects or ends a terminal goes through
//! here, so the registry has one writer and the host is never asked to do two
//! contradictory things at once.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use pushos_domain::ids::{ExecutionId, SessionId, WorkspaceId};
use pushos_domain::ports::{
    TerminalError, TerminalEvent, TerminalHost, TerminalSize, TerminalSpec, WorkspaceContext,
};
use pushos_domain::terminal::TerminalTarget;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::registry::TerminalRegistry;
use crate::session::TerminalSummary;

/// The shell used when a request does not name a program.
const FALLBACK_SHELL: &str = "/bin/zsh";

/// What to open.
///
/// Everything optional is filled in from how PushOS is configured, so a binding
/// can say no more than "open the tests terminal".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenTerminal {
    /// What the operator will call it, and what a binding will name.
    pub name: String,
    /// What to run. Defaults to the operator's shell.
    pub program: Option<String>,
    /// Its arguments.
    pub args: Vec<String>,
    /// Where to run it. Defaults to the workspace root.
    pub cwd: Option<PathBuf>,
    /// Which workspace it belongs to.
    pub workspace: Option<WorkspaceId>,
}

impl OpenTerminal {
    /// A request to open a terminal called `name`.
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

/// Runs and remembers terminals.
#[derive(Debug)]
pub struct TerminalSupervisor {
    host: Arc<dyn TerminalHost>,
    registry: Mutex<TerminalRegistry>,
    /// Which project is in effect, which is where a terminal starts when the
    /// request does not say.
    workspaces: Arc<dyn WorkspaceContext>,
    /// What a terminal runs when the request does not say.
    shell: String,
}

impl TerminalSupervisor {
    /// Builds a supervisor over `host`.
    pub fn new(host: Arc<dyn TerminalHost>, workspaces: Arc<dyn WorkspaceContext>) -> Self {
        Self {
            host,
            registry: Mutex::new(TerminalRegistry::new()),
            workspaces,
            shell: default_shell(),
        }
    }

    /// Sets the program a terminal runs when a request does not name one.
    #[must_use]
    pub fn with_shell(mut self, shell: impl Into<String>) -> Self {
        self.shell = shell.into();
        self
    }

    /// Opens a terminal, or selects the one already holding that name.
    ///
    /// A pad pressed twice must not start two of anything. Reusing a running
    /// terminal is what makes a physical control safe to lean on.
    pub async fn open(&self, request: OpenTerminal) -> Result<TerminalSummary, TerminalError> {
        if let Some(existing) = self.reuse(&request.name).await {
            debug!(name = %request.name, "a terminal by that name is already running");
            return Ok(existing);
        }

        let id = SessionId::new(format!("term-{}", short(&ExecutionId::generate())));
        let spec = self.specify(&request).await;
        let workspace = request.workspace.clone();

        let handle = self.host.open(id, spec.clone()).await?;
        info!(id = %handle.id, name = %spec.name, program = %spec.program, "terminal opened");

        let mut registry = self.registry.lock().await;
        Ok(registry.opened(&handle, &spec, workspace, Instant::now()))
    }

    /// Types into a terminal, exactly as given.
    pub async fn write(
        &self,
        target: &TerminalTarget,
        text: &str,
    ) -> Result<SessionId, TerminalError> {
        let id = self.require_live(target).await?;
        self.host.write(&id, text).await?;
        Ok(id)
    }

    /// Types a line and presses return.
    pub async fn run(
        &self,
        target: &TerminalTarget,
        line: &str,
    ) -> Result<SessionId, TerminalError> {
        // Trimmed so a stray newline in configuration does not submit twice.
        self.write(
            target,
            &format!("{}\r", line.trim_end_matches(['\r', '\n'])),
        )
        .await
    }

    /// Makes a terminal the one unqualified actions act on.
    pub async fn select(&self, target: &TerminalTarget) -> Result<SessionId, TerminalError> {
        let mut registry = self.registry.lock().await;
        let id = registry.resolve(target).ok_or_else(|| missing(target))?;
        registry.select(&id);
        Ok(id)
    }

    /// Ends a terminal.
    ///
    /// The exit is reported through the observer like any other, so the record
    /// is updated in one place whether the program stopped or was stopped.
    pub async fn close(&self, target: &TerminalTarget) -> Result<SessionId, TerminalError> {
        let id = self.require_live(target).await?;
        // Recorded before the host is asked, because the exit can come back
        // before this call returns.
        self.registry.lock().await.stopping(&id);
        self.host.close(&id).await?;
        Ok(id)
    }

    /// Tells a terminal it has been resized.
    pub async fn resize(
        &self,
        target: &TerminalTarget,
        size: TerminalSize,
    ) -> Result<SessionId, TerminalError> {
        let id = self.require_live(target).await?;
        self.host.resize(&id, size).await?;
        Ok(id)
    }

    /// Applies something a terminal did.
    ///
    /// Driven by the runtime as events arrive from the host.
    pub async fn apply(
        &self,
        terminal: &SessionId,
        event: &TerminalEvent,
    ) -> Option<TerminalSummary> {
        let mut registry = self.registry.lock().await;
        registry.apply(terminal, event, Instant::now())
    }

    /// Every terminal, in the order they were opened.
    pub async fn summaries(&self) -> Vec<TerminalSummary> {
        self.registry.lock().await.summaries()
    }

    /// How many terminals are running.
    pub async fn live_count(&self) -> usize {
        self.registry.lock().await.live_count()
    }

    /// Ends every running terminal.
    ///
    /// PushOS started these processes, so PushOS stops them rather than
    /// orphaning them when it exits.
    pub async fn shutdown(&self) {
        let live: Vec<SessionId> = {
            let registry = self.registry.lock().await;
            registry
                .summaries()
                .into_iter()
                .filter(|summary| summary.status.is_live())
                .map(|summary| summary.id)
                .collect()
        };

        for id in live {
            self.registry.lock().await.stopping(&id);
            if let Err(error) = self.host.close(&id).await {
                warn!(%id, %error, "a terminal did not stop cleanly");
            }
        }
    }

    /// The running terminal with this name, if there is one.
    async fn reuse(&self, name: &str) -> Option<TerminalSummary> {
        let mut registry = self.registry.lock().await;
        let id = registry.resolve(&TerminalTarget::Named(name.to_owned()))?;
        let session = registry.get(&id)?;
        if !session.is_live() {
            return None;
        }
        registry.select(&id);
        registry.get(&id).map(|session| session.summary(true))
    }

    /// Resolves a target, refusing one that has finished.
    async fn require_live(&self, target: &TerminalTarget) -> Result<SessionId, TerminalError> {
        let registry = self.registry.lock().await;
        let id = registry.resolve(target).ok_or_else(|| missing(target))?;

        match registry.get(&id) {
            Some(session) if session.is_live() => Ok(id),
            // Typing into a finished terminal would go nowhere and report
            // success, which is worse than saying so.
            _ => Err(TerminalError::Finished { terminal: id }),
        }
    }

    /// Fills a request in from the project in effect.
    ///
    /// A terminal opened while a project is selected opens in that project,
    /// with its environment. That is what makes one pad mean the whole thing.
    async fn specify(&self, request: &OpenTerminal) -> TerminalSpec {
        let program = request
            .program
            .clone()
            .unwrap_or_else(|| self.shell.clone());

        let cwd = match &request.cwd {
            Some(cwd) => cwd.clone(),
            None => self.workspaces.root(request.workspace.as_ref()).await,
        };

        TerminalSpec {
            name: request.name.clone(),
            program,
            args: request.args.clone(),
            cwd,
            env: self
                .workspaces
                .environment(request.workspace.as_ref())
                .await,
            size: TerminalSize::DEFAULT,
        }
    }
}

fn missing(target: &TerminalTarget) -> TerminalError {
    TerminalError::NoSuchTerminal {
        terminal: SessionId::new(target.to_string()),
    }
}

/// The last part of an identifier, which is enough to tell terminals apart.
fn short(id: &ExecutionId) -> String {
    let text = id.to_string();
    text.rsplit('-').next().unwrap_or(&text).to_owned()
}

/// The operator's shell, falling back to one that exists on every Mac.
fn default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| FALLBACK_SHELL.to_owned())
}

#[cfg(test)]
mod tests {
    use pushos_domain::error::ErrorClass;
    use pushos_domain::ports::TerminalStatus;
    use pushos_testkit::{FakeTerminal, RecordingTerminalObserver};

    use super::*;

    struct Fixture {
        supervisor: TerminalSupervisor,
        host: FakeTerminal,
        observer: RecordingTerminalObserver,
    }

    impl Fixture {
        fn new() -> Self {
            let observer = RecordingTerminalObserver::new();
            let host = FakeTerminal::new(Arc::new(observer.clone()));
            let supervisor = TerminalSupervisor::new(
                Arc::new(host.clone()),
                Arc::new(pushos_testkit::FixedRoot::new("/tmp")),
            )
            .with_shell("/bin/zsh");
            Self {
                supervisor,
                host,
                observer,
            }
        }

        /// Applies everything the host reported, as the runtime would.
        async fn drain(&self) {
            for (id, event) in self.observer.events() {
                self.supervisor.apply(&id, &event).await;
            }
        }
    }

    fn named(name: &str) -> TerminalTarget {
        TerminalTarget::Named(name.to_owned())
    }

    #[tokio::test]
    async fn opening_a_terminal_records_it_and_selects_it() {
        let fixture = Fixture::new();
        let summary = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");

        assert_eq!(summary.name, "tests");
        assert!(summary.selected);
        assert_eq!(summary.status, TerminalStatus::Starting);
        assert_eq!(fixture.supervisor.live_count().await, 1);
    }

    #[tokio::test]
    async fn a_terminal_with_no_program_runs_the_shell_in_the_root() {
        let fixture = Fixture::new();
        let summary = fixture
            .supervisor
            .open(OpenTerminal::named("shell"))
            .await
            .expect("the fake succeeds by default");

        let spec = fixture.host.spec(&summary.id).expect("it was opened");
        assert_eq!(spec.program, "/bin/zsh");
        assert_eq!(spec.cwd, PathBuf::from("/tmp"));
    }

    #[tokio::test]
    async fn a_request_that_names_a_program_gets_that_program() {
        let fixture = Fixture::new();
        let summary = fixture
            .supervisor
            .open(OpenTerminal {
                name: "tests".to_owned(),
                program: Some("cargo".to_owned()),
                args: vec!["test".to_owned()],
                cwd: Some(PathBuf::from("/usr")),
                workspace: None,
            })
            .await
            .expect("the fake succeeds by default");

        let spec = fixture.host.spec(&summary.id).expect("it was opened");
        assert_eq!(spec.program, "cargo");
        assert_eq!(spec.args, ["test"]);
        assert_eq!(spec.cwd, PathBuf::from("/usr"));
    }

    #[tokio::test]
    async fn opening_a_name_that_is_already_running_does_not_start_a_second_one() {
        // A pad is going to be pressed twice. It must not fork.
        let fixture = Fixture::new();
        let first = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");
        let again = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");

        assert_eq!(first.id, again.id);
        assert_eq!(fixture.host.open_count(), 1);
    }

    #[tokio::test]
    async fn a_name_freed_by_a_terminal_finishing_can_be_used_again() {
        let fixture = Fixture::new();
        let first = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");
        fixture.host.finish(&first.id, Some(0));
        fixture.drain().await;

        let second = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");
        assert_ne!(first.id, second.id);
    }

    #[tokio::test]
    async fn typing_reaches_the_terminal_a_name_resolves_to() {
        let fixture = Fixture::new();
        let summary = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");

        fixture
            .supervisor
            .write(&named("tests"), "hello")
            .await
            .expect("it is running");
        assert_eq!(fixture.host.typed(&summary.id).as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn running_a_line_presses_return_exactly_once() {
        let fixture = Fixture::new();
        let summary = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");

        // Configuration written by hand often ends with a newline already.
        fixture
            .supervisor
            .run(&named("tests"), "cargo test\n")
            .await
            .expect("it is running");
        assert_eq!(
            fixture.host.typed(&summary.id).as_deref(),
            Some("cargo test\r")
        );
    }

    #[tokio::test]
    async fn an_unqualified_action_goes_to_the_selected_terminal() {
        let fixture = Fixture::new();
        fixture
            .supervisor
            .open(OpenTerminal::named("build"))
            .await
            .expect("the fake succeeds by default");
        let latest = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");

        let target = fixture
            .supervisor
            .write(&TerminalTarget::Selected, "x")
            .await
            .expect("something is selected");
        assert_eq!(target, latest.id);
    }

    #[tokio::test]
    async fn selecting_moves_where_unqualified_actions_go() {
        let fixture = Fixture::new();
        let build = fixture
            .supervisor
            .open(OpenTerminal::named("build"))
            .await
            .expect("the fake succeeds by default");
        fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");

        fixture
            .supervisor
            .select(&named("build"))
            .await
            .expect("it exists");
        let target = fixture
            .supervisor
            .write(&TerminalTarget::Selected, "x")
            .await
            .expect("something is selected");
        assert_eq!(target, build.id);
    }

    #[tokio::test]
    async fn typing_into_a_terminal_that_has_finished_is_refused() {
        // Reporting success for text that went nowhere is worse than failing.
        let fixture = Fixture::new();
        let summary = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");
        fixture.host.finish(&summary.id, Some(0));
        fixture.drain().await;

        let error = fixture
            .supervisor
            .write(&named("tests"), "x")
            .await
            .expect_err("it has finished");
        assert!(matches!(error, TerminalError::Finished { .. }));
    }

    #[tokio::test]
    async fn naming_a_terminal_that_does_not_exist_is_refused() {
        let fixture = Fixture::new();
        let error = fixture
            .supervisor
            .write(&named("nothing"), "x")
            .await
            .expect_err("nothing is open");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn closing_ends_the_terminal_and_the_exit_comes_back_through_the_observer() {
        let fixture = Fixture::new();
        let summary = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");

        fixture
            .supervisor
            .close(&named("tests"))
            .await
            .expect("it is running");
        fixture.drain().await;

        assert_eq!(fixture.host.closed(), [summary.id]);
        assert_eq!(fixture.supervisor.live_count().await, 0);
    }

    #[tokio::test]
    async fn output_moves_a_terminal_from_starting_to_running() {
        let fixture = Fixture::new();
        let summary = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");

        fixture.host.emit(&summary.id, "compiling\n");
        fixture.drain().await;

        let listed = fixture.supervisor.summaries().await;
        assert_eq!(listed[0].status, TerminalStatus::Running);
        assert_eq!(listed[0].last_line.as_deref(), Some("compiling"));
    }

    #[tokio::test]
    async fn shutting_down_ends_every_running_terminal() {
        // PushOS started these processes; leaving them behind would be a leak
        // the operator has to clean up by hand.
        let fixture = Fixture::new();
        for name in ["build", "tests", "server"] {
            fixture
                .supervisor
                .open(OpenTerminal::named(name))
                .await
                .expect("the fake succeeds by default");
        }

        fixture.supervisor.shutdown().await;
        assert_eq!(fixture.host.closed().len(), 3);
        assert_eq!(fixture.host.open_count(), 0);
    }

    #[tokio::test]
    async fn closing_reports_a_stop_rather_than_a_failure() {
        let fixture = Fixture::new();
        fixture
            .supervisor
            .open(OpenTerminal::named("server"))
            .await
            .expect("the fake succeeds by default");

        fixture
            .supervisor
            .close(&named("server"))
            .await
            .expect("it is running");
        fixture.drain().await;

        let listed = fixture.supervisor.summaries().await;
        assert_eq!(listed[0].status, TerminalStatus::Stopped);
        assert!(!listed[0].status.is_failure());
    }

    #[tokio::test]
    async fn a_host_that_refuses_to_open_leaves_nothing_recorded() {
        let fixture = Fixture::new();
        fixture.host.fail_with(ErrorClass::ComponentFailure);

        let error = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect_err("the fake was told to fail");
        assert_eq!(error.class(), ErrorClass::ComponentFailure);
        assert!(fixture.supervisor.summaries().await.is_empty());
    }

    #[tokio::test]
    async fn resizing_reaches_the_host() {
        let fixture = Fixture::new();
        let summary = fixture
            .supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");

        let size = TerminalSize {
            columns: 200,
            rows: 60,
        };
        fixture
            .supervisor
            .resize(&named("tests"), size)
            .await
            .expect("it is running");
        assert_eq!(fixture.host.size(&summary.id), Some(size));
    }

    #[test]
    fn a_shortened_identifier_is_stable_and_not_empty() {
        let id = ExecutionId::generate();
        let shortened = short(&id);
        assert!(!shortened.is_empty());
        assert!(id.to_string().ends_with(&shortened));
    }
}
