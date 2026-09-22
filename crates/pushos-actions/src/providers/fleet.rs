//! Operating a whole fleet at once.
//!
//! The `agent` provider acts on one agent: the one filling a role, the one
//! selected, the one in a place on the grid. That is the right shape for a
//! pad. It is the wrong shape for sixty-four, where the operator's unit of
//! work is the fleet — hire twenty, give each of them a different job, send
//! them all home — and doing that one pad at a time would mean sixty-four
//! presses to start and sixty-four more to brief.
//!
//! Briefing is the interesting one, and the reason this is a provider rather
//! than a loop at a call site: the jobs come from a file, a line each, and
//! each agent gets the next line. That is what makes sixty-four agents doing
//! sixty-four different things something an operator can set up in one press
//! rather than an afternoon.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pushos_agents::AgentSupervisor;
use pushos_domain::action::{ActionContext, ActionResult, DisplayIntent};
use pushos_domain::agent::AgentTarget;
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::{ActionVerb, AgentId, ProviderName};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::ActionProvider;
use pushos_domain::ports::ProviderCapabilities;
use tracing::{info, warn};

/// The namespace this provider claims.
pub const NAMESPACE: &str = "fleet";

/// The largest fleet one press may hire.
///
/// One per pad. Past that the surface could not show what it had started,
/// which is the same as not knowing what is running.
const MOST: usize = 64;

/// The largest briefing file that will be read.
///
/// Sixty-four jobs is sixty-four lines; a megabyte is far more than that and
/// far less than a file that was pointed at by mistake.
const LONGEST_BRIEF: u64 = 1_048_576;

/// Hires, briefs and dismisses agents in bulk.
#[derive(Debug)]
pub struct FleetProvider {
    agents: Arc<AgentSupervisor>,
}

impl FleetProvider {
    /// Builds the provider over the supervisor that owns the sessions.
    pub const fn new(agents: Arc<AgentSupervisor>) -> Self {
        Self { agents }
    }

    /// Opens `count` sessions for a role.
    async fn hire(&self, context: &ActionContext) -> Result<ActionResult, ActionError> {
        let role: AgentId = context.params().require_text("role")?.into();
        let count = wanted(context)?;
        let workspace = context.surface.workspace.clone();

        let mut opened = 0;
        let mut refused = None;
        for _ in 0..count {
            match self.agents.recruit(&role, workspace.as_ref()).await {
                Ok(session) => {
                    info!(%session.id, %role, "hired into the fleet");
                    opened += 1;
                }
                // Whatever has been opened stays open: a fleet that half
                // started is a fleet the operator can still work with, and
                // tearing it down would throw away agents that are fine.
                Err(error) => {
                    warn!(%role, %error, "the fleet stopped growing");
                    refused = Some(error);
                    break;
                }
            }
        }

        if opened == 0 {
            return Err(refused.map_or_else(
                || nothing_happened("no agent could be hired"),
                |error| ActionError::backend(error.to_string(), error.class(), error),
            ));
        }

        let said = match refused {
            Some(error) => format!("{opened} of {count} hired; {error}"),
            None => format!("{opened} hired"),
        };
        Ok(reported(&role.to_string(), said))
    }

    /// Gives every agent in the fleet the next job in the file.
    async fn brief(&self, context: &ActionContext) -> Result<ActionResult, ActionError> {
        let jobs = read_jobs(context.params().require_text("jobs")?)?;
        if jobs.is_empty() {
            return Err(nothing_happened("the briefing has no jobs in it"));
        }

        let fleet = self.agents.sessions().await;
        let mut ordered: Vec<_> = fleet.iter().filter(|session| session.is_live()).collect();
        // The same order the pads are in, so the job on line three goes to the
        // agent on the third pad and an operator can follow along.
        ordered.sort_by(|left, right| {
            left.started_at
                .cmp(&right.started_at)
                .then_with(|| left.id.as_str().cmp(right.id.as_str()))
        });

        if ordered.is_empty() {
            return Err(nothing_happened("no agent is open to brief"));
        }

        let mut sent = 0;
        for (session, job) in ordered.iter().zip(jobs.iter()) {
            let target = AgentTarget::Session(session.id.clone());
            match self.agents.prompt(&target, job).await {
                Ok(_) => sent += 1,
                // One agent refusing its job is not a reason the other
                // sixty-three should go without theirs.
                Err(error) => warn!(%session.id, %error, "could not be briefed"),
            }
        }

        if sent == 0 {
            return Err(nothing_happened("no agent took its job"));
        }

        let spare = jobs.len().saturating_sub(ordered.len());
        let said = if spare > 0 {
            format!("{sent} briefed, {spare} jobs left over")
        } else {
            format!("{sent} briefed")
        };
        Ok(reported("Fleet", said))
    }

    /// Puts every open agent away.
    async fn dismiss(&self) -> Result<ActionResult, ActionError> {
        let fleet = self.agents.sessions().await;
        let live: Vec<_> = fleet.iter().filter(|session| session.is_live()).collect();
        if live.is_empty() {
            return Err(nothing_happened("no agent is open"));
        }

        let mut stopped = 0;
        for session in &live {
            let target = AgentTarget::Session(session.id.clone());
            match self.agents.stop(&target).await {
                Ok(_) => stopped += 1,
                Err(error) => warn!(%session.id, %error, "would not stop"),
            }
        }

        Ok(reported("Fleet", format!("{stopped} put away")))
    }
}

/// How many to hire, refusing a number the surface could not show.
fn wanted(context: &ActionContext) -> Result<usize, ActionError> {
    let count = context
        .params()
        .get("count")
        .map_or(Some(1), |value| {
            value.as_integer().and_then(|n| usize::try_from(n).ok())
        })
        .filter(|count| *count >= 1)
        .ok_or_else(|| nothing_happened("`count` must be a whole number, at least one"))?;

    if count > MOST {
        return Err(nothing_happened(format!(
            "a fleet of {count} is more than the {MOST} pads that could show it"
        )));
    }
    Ok(count)
}

/// Expands a leading `~`, the way an operator writes a path in configuration.
///
/// Done here rather than taken from the configuration crate because a provider
/// depends on domain ports, never on how PushOS reads its own files.
fn expand_home(written: &str) -> PathBuf {
    match written.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map_or_else(
            || PathBuf::from(written),
            |home| PathBuf::from(home).join(rest),
        ),
        None => PathBuf::from(written),
    }
}

/// Reads a briefing file: one job a line, blank lines and `#` comments ignored.
fn read_jobs(written: &str) -> Result<Vec<String>, ActionError> {
    let path = expand_home(written);

    let long = std::fs::metadata(&path)
        .map_err(|error| {
            ActionError::backend(
                format!("`{}` could not be read: {error}", path.display()),
                ErrorClass::Validation,
                error,
            )
        })?
        .len();
    if long > LONGEST_BRIEF {
        return Err(nothing_happened(format!(
            "`{}` is {long} bytes, which is not a list of jobs",
            path.display()
        )));
    }

    let text = std::fs::read_to_string(&path).map_err(|error| {
        ActionError::backend(
            format!("`{}` could not be read: {error}", path.display()),
            ErrorClass::Validation,
            error,
        )
    })?;

    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .take(MOST)
        .map(str::to_owned)
        .collect())
}

fn reported(title: &str, detail: String) -> ActionResult {
    ActionResult::completed()
        .with_message(detail.clone())
        .with_display(DisplayIntent::Toast {
            title: title.to_owned(),
            detail: Some(detail),
        })
}

fn nothing_happened(said: impl Into<String>) -> ActionError {
    let said = said.into();
    ActionError::backend(
        said.clone(),
        ErrorClass::Validation,
        std::io::Error::other(said),
    )
}

#[async_trait]
impl ActionProvider for FleetProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        // Every one of these starts or stops programs that write files and run
        // commands, which is the same decision as starting one agent.
        ProviderCapabilities::new(["hire", "brief", "dismiss"].map(ActionVerb::new))
            .requiring([Permission::ShellExecute])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        match context.definition.selector.verb.as_str() {
            "hire" => self.hire(&context).await,
            "brief" => self.brief(&context).await,
            "dismiss" => self.dismiss().await,
            verb => Err(ActionError::UnknownVerb {
                provider: self.name(),
                verb: verb.to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use pushos_agents::{AgentRoster, SilentObserver};
    use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
    use pushos_domain::agent::AgentDefinition;
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::ids::CorrelationId;
    use pushos_testkit::FakeAgent;

    use super::*;

    fn supervisor(agent: &FakeAgent) -> Arc<AgentSupervisor> {
        let mut roster = AgentRoster::new();
        let mut worker = AgentDefinition::new("worker", "Worker");
        worker.objective = "work".to_owned();
        roster.define(worker);
        roster.install(Arc::new(agent.clone()));

        Arc::new(AgentSupervisor::new(
            Arc::new(roster),
            Arc::new(SilentObserver),
            Arc::new(pushos_testkit::FixedRoot::new("/tmp/project")),
        ))
    }

    async fn run(
        supervisor: &Arc<AgentSupervisor>,
        verb: &str,
        params: Params,
    ) -> Result<ActionResult, ActionError> {
        let definition = ActionDefinition::new(ActionSelector::new(NAMESPACE, verb), params);
        FleetProvider::new(Arc::clone(supervisor))
            .execute(ActionContext::new(
                definition,
                CorrelationId::generate(),
                SurfaceContext::empty(),
            ))
            .await
    }

    fn hiring(count: i64) -> Params {
        let mut params = Params::new();
        params.set("role", ParamValue::Text("worker".into()));
        params.set("count", ParamValue::Integer(count));
        params
    }

    /// A briefing file that cleans itself up.
    struct Brief(PathBuf);

    impl Brief {
        fn of(text: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "pushos-brief-{}",
                pushos_domain::ids::ExecutionId::generate()
            ));
            std::fs::write(&path, text).expect("the temporary directory is writable");
            Self(path)
        }

        fn params(&self) -> Params {
            let mut params = Params::new();
            params.set(
                "jobs",
                ParamValue::Text(self.0.to_string_lossy().as_ref().into()),
            );
            params
        }
    }

    impl Drop for Brief {
        fn drop(&mut self) {
            std::fs::remove_file(&self.0).ok();
        }
    }

    #[tokio::test]
    async fn a_fleet_is_hired_in_one_press() {
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);

        let result = run(&supervisor, "hire", hiring(20)).await.expect("hires");

        assert_eq!(supervisor.sessions().await.len(), 20);
        assert_eq!(result.message.as_deref(), Some("20 hired"));
    }

    #[tokio::test]
    async fn every_agent_in_a_fleet_is_its_own_session() {
        // The thing that separates a fleet from a role: asking for the role
        // twice finds one agent, and hiring twice makes two.
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);
        run(&supervisor, "hire", hiring(5)).await.expect("hires");

        let sessions = supervisor.sessions().await;
        let mut ids: Vec<_> = sessions.iter().map(|s| s.id.as_str()).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "every agent should have its own session");
    }

    #[tokio::test]
    async fn a_fleet_larger_than_the_surface_is_refused_before_anything_starts() {
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);

        let refused = run(&supervisor, "hire", hiring(65))
            .await
            .expect_err("more than there are pads");
        assert!(refused.to_string().contains("64 pads"));
        assert!(
            supervisor.sessions().await.is_empty(),
            "nothing should have been started"
        );
    }

    #[tokio::test]
    async fn a_count_that_is_not_a_number_of_agents_is_refused() {
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);
        for count in [0, -3] {
            assert!(
                run(&supervisor, "hire", hiring(count)).await.is_err(),
                "{count} is not a fleet"
            );
        }
        assert!(supervisor.sessions().await.is_empty());
    }

    #[tokio::test]
    async fn each_agent_gets_the_job_on_its_own_line() {
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);
        run(&supervisor, "hire", hiring(3)).await.expect("hires");

        let brief =
            Brief::of("fix the flaky test\n# a comment\n\nupdate the README\nbump the deps\n");
        let result = run(&supervisor, "brief", brief.params())
            .await
            .expect("briefs");

        assert_eq!(result.message.as_deref(), Some("3 briefed"));
        let sent: Vec<String> = agent
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                pushos_testkit::AgentCall::Prompted { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(
            sent,
            vec![
                "fix the flaky test".to_owned(),
                "update the README".to_owned(),
                "bump the deps".to_owned(),
            ],
            "blank lines and comments are not jobs, and order follows the file"
        );
    }

    #[tokio::test]
    async fn jobs_with_nobody_to_do_them_are_reported_rather_than_dropped_quietly() {
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);
        run(&supervisor, "hire", hiring(2)).await.expect("hires");

        let brief = Brief::of("one\ntwo\nthree\nfour\n");
        let result = run(&supervisor, "brief", brief.params())
            .await
            .expect("briefs");

        assert_eq!(
            result.message.as_deref(),
            Some("2 briefed, 2 jobs left over")
        );
    }

    #[tokio::test]
    async fn briefing_nobody_says_so_rather_than_reporting_success() {
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);
        let brief = Brief::of("something to do\n");

        let refused = run(&supervisor, "brief", brief.params())
            .await
            .expect_err("there is no fleet");
        assert!(refused.to_string().contains("no agent is open"));
    }

    #[tokio::test]
    async fn an_empty_briefing_is_refused() {
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);
        run(&supervisor, "hire", hiring(1)).await.expect("hires");

        let brief = Brief::of("# nothing but comments\n\n");
        let refused = run(&supervisor, "brief", brief.params())
            .await
            .expect_err("no jobs");
        assert!(refused.to_string().contains("no jobs in it"));
    }

    #[tokio::test]
    async fn a_briefing_that_is_not_there_says_which_file() {
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);
        run(&supervisor, "hire", hiring(1)).await.expect("hires");

        let mut params = Params::new();
        params.set("jobs", ParamValue::Text("/nowhere/at/all/jobs.txt".into()));
        let refused = run(&supervisor, "brief", params)
            .await
            .expect_err("no such file");
        assert!(refused.to_string().contains("/nowhere/at/all/jobs.txt"));
    }

    #[tokio::test]
    async fn a_fleet_is_dismissed_in_one_press() {
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);
        run(&supervisor, "hire", hiring(6)).await.expect("hires");

        let result = run(&supervisor, "dismiss", Params::new())
            .await
            .expect("dismisses");
        assert_eq!(result.message.as_deref(), Some("6 put away"));
    }

    #[tokio::test]
    async fn dismissing_an_empty_fleet_says_so() {
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);
        let refused = run(&supervisor, "dismiss", Params::new())
            .await
            .expect_err("nothing to dismiss");
        assert!(refused.to_string().contains("no agent is open"));
    }

    #[tokio::test]
    async fn an_unknown_verb_is_refused() {
        let agent = FakeAgent::new("codex");
        let supervisor = supervisor(&agent);
        assert!(run(&supervisor, "mutiny", Params::new()).await.is_err());
    }
}
