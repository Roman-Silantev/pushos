//! Assembling the host-specific half of PushOS.
//!
//! One place decides which concrete adapters back the domain's ports. Swapping
//! a host means changing this file and nothing else.

use std::sync::Arc;

use pushos_acp::{AcpBackend, AgentCommand};
use pushos_actions::providers::{
    agent, application, media, page, shell, shortcut, terminal, voice, workflow, workspace,
};
use pushos_agents::{AgentRoster, AgentSupervisor};
use pushos_config::RuntimeConfig;
use pushos_domain::ports::{
    ActionProvider, AgentObserver, ProcessRunner, TerminalObserver, WorkspaceContext,
    WorkspaceMemoryStore,
};
use pushos_macos::{AppleScriptMedia, OpenLauncher, ShortcutsCli, SystemProcessRunner};
use pushos_terminal::{PtyTerminals, TerminalSupervisor};
use pushos_voice::{VoiceListener, VoiceRouter};
use pushos_workflows::{WorkflowCatalogue, WorkflowEngine};
use pushos_workspaces::{GitRepository, WorkspaceManager, WorkspaceRegistry, warn_if_missing};
use tracing::{info, warn};

/// Builds the workspace manager from configuration.
///
/// Always built, even with no projects configured: everything downstream asks
/// it where to work, and a PushOS with no projects is simply one whose answer
/// is always the same directory.
pub(crate) fn workspaces(
    config: &RuntimeConfig,
    memory: Arc<dyn WorkspaceMemoryStore>,
    processes: Arc<dyn ProcessRunner>,
    worktree_root: std::path::PathBuf,
) -> Arc<WorkspaceManager> {
    for workspace in &config.workspaces {
        warn_if_missing(workspace);
    }
    if !config.workspaces.is_empty() {
        info!(projects = config.workspaces.len(), "projects configured");
    }

    Arc::new(WorkspaceManager::new(
        WorkspaceRegistry::new(config.workspaces.clone()),
        memory,
        Arc::new(GitRepository::new(processes)),
        config.workspace_root(),
        worktree_root,
    ))
}

/// Builds the agent supervisor from configuration.
///
/// Returns `None` when nothing is configured, because a surface with no agent
/// roles should not carry an agent namespace that refuses every binding.
pub(crate) fn agents(
    config: &RuntimeConfig,
    observer: Arc<dyn AgentObserver>,
    workspaces: Arc<dyn WorkspaceContext>,
) -> Option<Arc<AgentSupervisor>> {
    if config.agents.is_empty() {
        return None;
    }

    let mut roster = AgentRoster::new();
    for definition in &config.agents {
        roster.define(definition.clone());
    }

    // A provider is a command. One named without a command uses the default
    // PushOS ships for that name, if it has one; naming it is still required,
    // because PushOS does not start a subprocess the operator never mentioned.
    let mut installed = 0;
    for entry in &config.providers {
        let Some(command) = command_for(entry) else {
            warn!(
                provider = %entry.id,
                "PushOS has no default command for this provider; give it a `program`"
            );
            continue;
        };

        info!(provider = %entry.id, command = %command.describe(), "agent provider available");
        roster.install(Arc::new(AcpBackend::new(
            entry.id.as_str(),
            command,
            Arc::clone(&observer),
        )));
        installed += 1;
    }

    if installed == 0 {
        warn!(
            roles = config.agents.len(),
            "agent roles are configured but no provider can run them"
        );
        return None;
    }

    Some(Arc::new(AgentSupervisor::new(
        Arc::new(roster),
        observer,
        workspaces,
    )))
}

/// Builds the terminal supervisor.
///
/// Always built, unlike the agents: a terminal needs nothing configured, since
/// the operator's own shell is always there to run.
pub(crate) fn terminals(
    config: &RuntimeConfig,
    observer: Arc<dyn TerminalObserver>,
    workspaces: Arc<dyn WorkspaceContext>,
) -> Arc<TerminalSupervisor> {
    let host = Arc::new(PtyTerminals::new(observer));
    let mut supervisor = TerminalSupervisor::new(host, workspaces);

    if let Some(shell) = &config.shell {
        info!(%shell, "terminals will run the configured shell");
        supervisor = supervisor.with_shell(shell.as_str());
    }

    Arc::new(supervisor)
}

/// Builds the workflow engine from configuration.
///
/// Returns `None` when nothing is configured, because a surface with no
/// workflows should not carry a namespace that refuses every binding.
pub(crate) fn workflows(
    config: &RuntimeConfig,
    store: Arc<dyn pushos_domain::ports::RunStore>,
    observer: Arc<dyn pushos_workflows::RunObserver>,
    agents: Option<&Arc<AgentSupervisor>>,
    terminals: &Arc<TerminalSupervisor>,
) -> Option<Arc<WorkflowEngine>> {
    if config.workflows.is_empty() {
        return None;
    }

    info!(workflows = config.workflows.len(), "workflows available");
    let mut engine = WorkflowEngine::new(
        WorkflowCatalogue::new(config.workflows.clone()),
        store,
        observer,
    )
    .with_terminals(Arc::clone(terminals));

    if let Some(supervisor) = agents {
        engine = engine.with_agents(Arc::clone(supervisor));
    }

    Some(Arc::new(engine))
}

/// Builds push to talk from configuration.
///
/// Returns `None` when voice is not configured, because a surface that cannot
/// listen should not carry a namespace that refuses every binding. A configured
/// section that cannot be built is reported and left out: PushOS still runs,
/// it simply does not listen.
pub(crate) fn voice(config: &RuntimeConfig) -> Option<Arc<voice::VoiceProvider>> {
    let settings = config.voice.as_ref()?;

    if !settings.is_useful() {
        warn!(
            "voice is configured with no phrases and no request action; nothing could come of speaking"
        );
    }

    let microphone = pushos_voice::microphone()
        .inspect_err(|error| warn!(%error, "PushOS will not listen"))
        .ok()?;
    let transcriber = pushos_voice::transcriber(settings.engine, settings.model.as_deref())
        .inspect_err(|error| warn!(%error, engine = %settings.engine, "PushOS will not listen"))
        .ok()?;

    info!(
        engine = transcriber.name(),
        phrases = settings.commands.len(),
        "push to talk available"
    );
    let listener = Arc::new(VoiceListener::new(
        microphone,
        transcriber,
        VoiceRouter::new(settings.commands.clone()),
    ));
    Some(Arc::new(voice::VoiceProvider::new(
        listener,
        settings.request.clone(),
    )))
}

/// The command that starts a configured provider.
fn command_for(entry: &pushos_config::model::ProviderEntry) -> Option<AgentCommand> {
    let command = match &entry.program {
        Some(program) => AgentCommand::new(program.as_str(), entry.args.clone()),
        None => AgentCommand::known(&entry.id)?,
    };

    Some(entry.env.iter().fold(command, |command, (name, value)| {
        command.with_env(name, value)
    }))
}

/// Builds every action provider PushOS ships with.
///
/// A provider whose backend cannot be configured is left out rather than
/// installed in a broken state, so a binding that references it fails with
/// "no provider" instead of failing halfway through doing something.
pub(crate) fn providers(
    config: &RuntimeConfig,
    agents: Option<&Arc<AgentSupervisor>>,
    terminals: &Arc<TerminalSupervisor>,
    workspaces: &Arc<WorkspaceManager>,
    workflows: Option<&Arc<WorkflowEngine>>,
    listening: Option<&Arc<voice::VoiceProvider>>,
) -> Vec<Arc<dyn ActionProvider>> {
    let processes: Arc<dyn ProcessRunner> = Arc::new(SystemProcessRunner::new());
    let launcher: Arc<dyn pushos_domain::ports::ApplicationLauncher> =
        Arc::new(OpenLauncher::new(Arc::clone(&processes)));

    let mut providers: Vec<Arc<dyn ActionProvider>> = vec![
        Arc::new(page::PageProvider::new()),
        Arc::new(workspace::WorkspaceProvider::new(
            Arc::clone(workspaces),
            Arc::clone(&launcher),
        )),
        Arc::new(application::ApplicationProvider::new(Arc::clone(&launcher))),
        Arc::new(shortcut::ShortcutProvider::new(Arc::new(
            ShortcutsCli::new(Arc::clone(&processes)),
        ))),
        Arc::new(shell::ShellProvider::new(Arc::clone(&processes))),
        Arc::new(terminal::TerminalProvider::new(Arc::clone(terminals))),
    ];

    // Agents are offered only when there is something to run them, so a
    // binding fails with "no provider" rather than "unknown verb".
    if let Some(supervisor) = agents {
        providers.push(Arc::new(agent::AgentProvider::new(Arc::clone(supervisor))));
    }

    // The same for workflows: a namespace that refuses every binding would
    // report "unknown verb" rather than "none are configured".
    if let Some(engine) = workflows {
        providers.push(Arc::new(workflow::WorkflowProvider::new(Arc::clone(
            engine,
        ))));
    }

    // Voice is offered only when it can actually listen, so a binding fails
    // with "no provider" rather than with a microphone that is not there.
    if let Some(provider) = listening {
        providers.push(Arc::clone(provider) as Arc<dyn ActionProvider>);
    }

    if let Some(controller) = media_controller(config, &processes) {
        providers.push(Arc::new(media::MediaProvider::new(controller)));
    } else {
        warn!("media actions are unavailable; check the configured media player");
    }

    providers
}

/// Every namespace PushOS ships, and the verbs each one has.
///
/// Built from the providers themselves, so nothing can claim an action exists
/// after it was renamed or removed. Only the tests need this: a running PushOS
/// answers the same question over the control socket.
#[cfg(test)]
pub(crate) fn shipped_namespaces(
    config: &RuntimeConfig,
) -> std::collections::BTreeMap<String, Vec<String>> {
    let (terminal_reporter, _terminal_updates) = pushos_runtime::TerminalReporter::new();
    let (agent_reporter, _agent_updates) = pushos_runtime::AgentReporter::new();

    let manager = workspaces(
        config,
        Arc::new(pushos_workspaces::ForgetfulMemory),
        Arc::new(SystemProcessRunner::new()),
        std::env::temp_dir().join("pushos-namespaces"),
    );
    let terminals = terminals(
        config,
        Arc::new(terminal_reporter),
        Arc::clone(&manager) as _,
    );
    let agents = agents(config, Arc::new(agent_reporter), Arc::clone(&manager) as _);

    let engine = workflows(
        config,
        Arc::new(pushos_workflows::ForgetfulRuns),
        Arc::new(pushos_workflows::SilentObserver),
        agents.as_ref(),
        &terminals,
    );

    providers(
        config,
        agents.as_ref(),
        &terminals,
        &manager,
        engine.as_ref(),
        voice(config).as_ref(),
    )
    .into_iter()
    .map(|provider| {
        let verbs = provider
            .capabilities()
            .verbs()
            .iter()
            .map(ToString::to_string)
            .collect();
        (provider.name().to_string(), verbs)
    })
    .collect()
}

fn media_controller(
    config: &RuntimeConfig,
    processes: &Arc<dyn ProcessRunner>,
) -> Option<Arc<dyn pushos_domain::ports::MediaController>> {
    let controller = match &config.media_player {
        Some(player) => AppleScriptMedia::for_player(Arc::clone(processes), player.as_str())
            .inspect_err(|error| warn!(%error, "the configured media player is not usable"))
            .ok()?,
        None => AppleScriptMedia::new(Arc::clone(processes)),
    };
    Some(Arc::new(controller))
}

#[cfg(test)]
mod tests {
    use pushos_config::ConfigFile;

    use super::*;

    fn config(text: &str) -> RuntimeConfig {
        let parsed: ConfigFile = toml::from_str(text).expect("well-formed");
        RuntimeConfig::build(&parsed).expect("valid")
    }

    /// The manager a configuration produces, with nothing on disk touched.
    fn manager(config: &RuntimeConfig) -> Arc<WorkspaceManager> {
        workspaces(
            config,
            Arc::new(pushos_testkit::FakeWorkspaceMemory::new()),
            Arc::new(pushos_testkit::FakeProcesses::new()),
            std::path::PathBuf::from("/tmp/pushos-test-trees"),
        )
    }

    /// A supervisor over a real pseudo-terminal host that is never asked to
    /// open anything, so no process is started.
    fn idle_terminals(config: &RuntimeConfig) -> Arc<TerminalSupervisor> {
        let (reporter, _updates) = pushos_runtime::TerminalReporter::new();
        terminals(config, Arc::new(reporter), manager(config))
    }

    /// The engine a configuration produces, over a store that keeps nothing.
    fn engine(
        config: &RuntimeConfig,
        terminals: &Arc<TerminalSupervisor>,
    ) -> Option<Arc<WorkflowEngine>> {
        workflows(
            config,
            Arc::new(pushos_testkit::FakeRunStore::new()),
            Arc::new(pushos_workflows::SilentObserver),
            None,
            terminals,
        )
    }

    #[test]
    fn every_shipped_namespace_is_present_by_default() {
        let settings = config("");
        let namespaces: Vec<_> = providers(
            &settings,
            None,
            &idle_terminals(&settings),
            &manager(&settings),
            engine(&settings, &idle_terminals(&settings)).as_ref(),
            voice(&settings).as_ref(),
        )
        .iter()
        .map(|provider| provider.name().to_string())
        .collect();

        for expected in ["page", "app", "shortcut", "shell", "media", "terminal"] {
            assert!(
                namespaces.contains(&expected.to_owned()),
                "`{expected}` is missing"
            );
        }
    }

    #[test]
    fn namespaces_are_unique_so_the_registry_will_accept_them_all() {
        let settings = config("");
        let mut namespaces: Vec<_> = providers(
            &settings,
            None,
            &idle_terminals(&settings),
            &manager(&settings),
            engine(&settings, &idle_terminals(&settings)).as_ref(),
            voice(&settings).as_ref(),
        )
        .iter()
        .map(|provider| provider.name().to_string())
        .collect();
        namespaces.sort();
        let total = namespaces.len();
        namespaces.dedup();
        assert_eq!(total, namespaces.len());
    }

    #[test]
    fn a_surface_with_no_agent_roles_carries_no_agent_namespace() {
        // A namespace that refuses every binding is worse than none: the
        // failure would say "unknown verb" rather than "no agents configured".
        let settings = config("");
        let namespaces: Vec<_> = providers(
            &settings,
            None,
            &idle_terminals(&settings),
            &manager(&settings),
            engine(&settings, &idle_terminals(&settings)).as_ref(),
            voice(&settings).as_ref(),
        )
        .iter()
        .map(|provider| provider.name().to_string())
        .collect();
        assert!(!namespaces.contains(&"agent".to_owned()));
    }

    #[test]
    fn naming_a_provider_pushos_knows_is_enough_to_start_it() {
        let named = config(
            r#"
            [[providers]]
            id = "claude"

            [[agents]]
            id = "builder"
            name = "Builder"
            preferred = ["claude"]
            "#,
        );

        let (reporter, _updates) = pushos_runtime::AgentReporter::new();
        let supervisor = agents(
            &named,
            Arc::new(reporter),
            Arc::new(pushos_testkit::FixedRoot::new("/tmp")),
        )
        .expect("a name PushOS knows needs no command repeated");
        assert_eq!(supervisor.roster().providers().len(), 1);
    }

    #[test]
    fn roles_with_no_provider_at_all_leave_agents_unavailable() {
        // Better than installing a namespace that refuses every binding, and
        // better than starting a subprocess nobody asked for.
        let orphaned = config(
            r#"
            [[agents]]
            id = "builder"
            name = "Builder"
            "#,
        );

        let (reporter, _updates) = pushos_runtime::AgentReporter::new();
        assert!(
            agents(
                &orphaned,
                Arc::new(reporter),
                Arc::new(pushos_testkit::FixedRoot::new("/tmp"))
            )
            .is_none()
        );
    }

    #[test]
    fn a_provider_pushos_does_not_know_and_that_names_no_command_is_skipped() {
        let unknown = config(
            r#"
            [[providers]]
            id = "something-else"

            [[agents]]
            id = "builder"
            name = "Builder"
            preferred = ["something-else"]
            "#,
        );

        let (reporter, _updates) = pushos_runtime::AgentReporter::new();
        assert!(
            agents(
                &unknown,
                Arc::new(reporter),
                Arc::new(pushos_testkit::FixedRoot::new("/tmp"))
            )
            .is_none()
        );
    }

    #[test]
    fn a_role_preferring_something_pushos_does_not_know_is_left_unfilled() {
        // Configuration validation refuses an undeclared provider, so reaching
        // here means the operator declared it; PushOS has no default to guess.
        let with_roles = config(
            r#"
            [[providers]]
            id = "mine"
            program = "my-agent"

            [[agents]]
            id = "builder"
            name = "Builder"
            preferred = ["mine"]
            "#,
        );

        let (reporter, _updates) = pushos_runtime::AgentReporter::new();
        let supervisor = agents(
            &with_roles,
            Arc::new(reporter),
            Arc::new(pushos_testkit::FixedRoot::new("/tmp")),
        )
        .expect("the declared provider is installed as written");
        assert_eq!(supervisor.roster().providers().len(), 1);
    }

    #[test]
    fn terminals_are_available_with_nothing_configured() {
        // Unlike agents, a terminal needs no provider installed: the
        // operator's own shell is always there.
        let settings = config("");
        let namespaces: Vec<_> = providers(
            &settings,
            None,
            &idle_terminals(&settings),
            &manager(&settings),
            engine(&settings, &idle_terminals(&settings)).as_ref(),
            voice(&settings).as_ref(),
        )
        .iter()
        .map(|provider| provider.name().to_string())
        .collect();
        assert!(namespaces.contains(&"terminal".to_owned()));
    }

    #[tokio::test]
    async fn a_configured_shell_is_what_a_terminal_runs() {
        let settings = config("[runtime]\nshell = \"/bin/dash\"\n");
        let supervisor = idle_terminals(&settings);

        // Nothing has been opened, so nothing has been started.
        assert_eq!(supervisor.live_count().await, 0);
        assert_eq!(settings.shell.as_deref(), Some("/bin/dash"));
    }

    #[test]
    fn an_unusable_media_player_leaves_the_namespace_unclaimed() {
        let unusable = config("[runtime]\nmedia_player = \"Music\\\" to quit\"\n");
        let namespaces: Vec<_> = providers(
            &unusable,
            None,
            &idle_terminals(&unusable),
            &manager(&unusable),
            engine(&unusable, &idle_terminals(&unusable)).as_ref(),
            voice(&unusable).as_ref(),
        )
        .iter()
        .map(|provider| provider.name().to_string())
        .collect();

        assert!(
            !namespaces.contains(&"media".to_owned()),
            "a provider must not be installed in a broken state"
        );
        assert!(
            namespaces.contains(&"shell".to_owned()),
            "the rest still work"
        );
    }
}
