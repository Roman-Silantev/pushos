//! Capturing and finding notes from the surface.
//!
//! One press to write down what was just said, one to look something up, one to
//! put what was found in front of the agent that needs it. Notes are files in
//! directories the operator chose, so nothing here owns them: PushOS is a way
//! into them, not the place they live.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use pushos_domain::action::{
    ActionContext, ActionDefinition, ActionResult, ActionSelector, ActionStatus, DisplayIntent,
    ParamValue, Params,
};
use pushos_domain::context::SurfaceContext;
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::{ActionVerb, ProviderName};
use pushos_domain::memory::{Draft, Excerpt, Search};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::{
    ActionProvider, ActionRunner, MemoryError, MemoryStore, ProviderCapabilities,
};
use tracing::{info, warn};

/// The namespace this provider claims.
pub const NAMESPACE: &str = "memory";

/// The parameter the words arrive under.
///
/// `text`, the same as everywhere else words are passed in PushOS, so a spoken
/// phrase can reach this without translating anything on the way.
const WORDS: &str = "text";

/// How many notes a briefing hands an agent.
///
/// Small on purpose: a briefing is context, and an agent given twenty notes has
/// been given a search result rather than a briefing.
const MOST_BRIEFED: usize = 3;

/// Turns gestures into notes.
#[derive(Debug)]
pub struct MemoryProvider {
    notes: Arc<dyn MemoryStore>,
    /// What a briefing is handed to, with the notes as `text`.
    brief: Option<ActionSelector>,
    /// How a briefing is actually delivered.
    ///
    /// Weak because the dispatcher owns this provider, and going through the
    /// dispatcher is the point: a briefing gets the same permission check
    /// pressing the agent's own pad would.
    actions: tokio::sync::Mutex<Weak<dyn ActionRunner>>,
}

impl MemoryProvider {
    /// Builds the provider over a store.
    pub fn new(notes: Arc<dyn MemoryStore>, brief: Option<ActionSelector>) -> Self {
        Self {
            notes,
            brief,
            actions: tokio::sync::Mutex::new(Weak::<NoActions>::new()),
        }
    }

    /// Gives the provider the dispatcher to run through.
    ///
    /// Given after construction, because the dispatcher this points at contains
    /// this provider.
    pub async fn use_actions(&self, runner: &Arc<dyn ActionRunner>) {
        *self.actions.lock().await = Arc::downgrade(runner);
    }

    /// Reads the sources and brings the index up to date.
    ///
    /// Run once at startup. Notes are files, and files change while PushOS is
    /// not running: an index that only caught up when a pad was pressed would
    /// be one nobody could trust, and a first search that found nothing would
    /// look like a fault.
    pub async fn reindex(&self) -> Result<usize, MemoryError> {
        self.notes.refresh().await
    }

    /// Builds the action that hands notes to whatever was configured.
    ///
    /// Public so a configuration can be checked against it rather than against
    /// a copy of what it is believed to do.
    pub fn briefing_for(brief: &ActionSelector, notes: &str) -> ActionDefinition {
        let mut params = Params::new();
        params.set(WORDS, ParamValue::Text(notes.into()));
        ActionDefinition::new(brief.clone(), params)
    }

    /// What the operator is looking for.
    ///
    /// The words, the project they are in unless they said otherwise, and how
    /// many results they can read.
    fn search_from(context: &ActionContext) -> Search {
        let params = context.params();
        let text = params
            .text(WORDS)
            .or_else(|| params.text("target"))
            .unwrap_or_default();

        let mut search = Search::for_text(text);
        search.tag = params.text("tag").map(ToOwned::to_owned);
        search.limit = params
            .text("limit")
            .and_then(|written| written.parse().ok())
            .unwrap_or(pushos_domain::memory::DEFAULT_LIMIT);

        // Scoped to where the operator is, because that is what they meant.
        // `workspace = "all"` is how they say otherwise.
        if params.text("workspace") != Some("all") {
            search.workspace.clone_from(&context.surface.workspace);
        }
        search
    }

    /// What to write down.
    fn draft_from(context: &ActionContext) -> Result<Draft, ActionError> {
        let params = context.params();
        let body = params
            .text(WORDS)
            .or_else(|| params.text("target"))
            .unwrap_or_default();

        let mut draft = Draft::new(body).about(context.surface.workspace.clone());
        if let Some(title) = params.text("title") {
            draft = draft.called(title);
        }
        draft.tags = params
            .text("tags")
            .map(|written| {
                written
                    .split([',', ' '])
                    .filter(|tag| !tag.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        if draft.is_empty() {
            return Err(invalid("a note needs something to say; pass it as `text`"));
        }
        Ok(draft)
    }

    /// Shows what was found, or says that nothing was.
    fn report(found: &[Excerpt], asked: &str) -> ActionResult {
        if found.is_empty() {
            return ActionResult {
                status: ActionStatus::Completed,
                message: Some("nothing found".to_owned()),
                display: Some(DisplayIntent::Toast {
                    title: "Nothing found".to_owned(),
                    detail: (!asked.is_empty()).then(|| asked.to_owned()),
                }),
            };
        }

        let kind = if asked.is_empty() {
            NAMESPACE.to_owned()
        } else {
            format!("{NAMESPACE} — {asked}")
        };
        ActionResult {
            status: ActionStatus::Completed,
            message: Some(format!("{} found", found.len())),
            display: Some(DisplayIntent::Report {
                kind,
                title: format!("{} found", found.len()),
                lines: found.iter().map(ToString::to_string).collect(),
            }),
        }
    }

    /// Hands what was found to whatever was configured to receive it.
    async fn brief(&self, found: &[Excerpt], surface: &SurfaceContext) -> ActionResult {
        let Some(selector) = &self.brief else {
            // Nowhere configured to send them. Showing them is still useful,
            // and inventing a destination would not be.
            return Self::report(found, "");
        };

        let mut written = Vec::with_capacity(found.len());
        for excerpt in found.iter().take(MOST_BRIEFED) {
            match self.notes.read(&excerpt.id).await {
                Ok(Some(note)) => written.push(format!("## {}\n\n{}", note.title, note.body)),
                // A note in the index but not on disk was deleted between the
                // search and now. The others are still worth sending.
                Ok(None) => warn!(note = %excerpt.id, "a note has gone since it was indexed"),
                Err(error) => warn!(%error, note = %excerpt.id, "a note could not be read"),
            }
        }

        if written.is_empty() {
            return Self::report(&[], "");
        }

        let notes = written.join("\n\n");
        let Some(runner) = self.actions.lock().await.upgrade() else {
            warn!("found notes with nothing left to hand them to");
            return ActionResult {
                status: ActionStatus::Failed,
                message: Some("memory is not connected to anything".to_owned()),
                display: None,
            };
        };

        info!(notes = written.len(), action = %selector, "briefing");
        match runner
            .run(Self::briefing_for(selector, &notes), surface.clone())
            .await
        {
            Ok(result) => ActionResult {
                status: result.status,
                message: result
                    .message
                    .or_else(|| Some(format!("{} notes sent", written.len()))),
                display: Some(DisplayIntent::Toast {
                    title: format!("{} notes sent", written.len()),
                    detail: found.first().map(|first| first.title.clone()),
                }),
            },
            Err(error) => {
                warn!(%error, "a briefing could not be delivered");
                ActionResult {
                    status: ActionStatus::Failed,
                    message: Some(error.to_string()),
                    display: Some(DisplayIntent::Toast {
                        title: "Briefing failed".to_owned(),
                        detail: Some(error.to_string()),
                    }),
                }
            }
        }
    }
}

#[async_trait]
impl ActionProvider for MemoryProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(
            ["write", "find", "recent", "brief", "refresh"].map(ActionVerb::new),
        )
        // Only the verb that puts a file on the operator's disk. Reading the
        // directories they configured as note sources is what configuring them
        // meant, and gating it would teach them to grant more than they meant.
        .verb_requiring(ActionVerb::new("write"), [Permission::FilesystemWrite])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        match context.definition.selector.verb.as_str() {
            "write" => {
                let draft = Self::draft_from(&context)?;
                let note = self.notes.write(&draft).await.map_err(into_action_error)?;
                Ok(ActionResult {
                    status: ActionStatus::Completed,
                    message: Some(note.title.clone()),
                    display: Some(DisplayIntent::Toast {
                        title: "Noted".to_owned(),
                        detail: Some(note.title),
                    }),
                })
            }

            "find" | "recent" => {
                let search = if context.definition.selector.verb.as_str() == "recent" {
                    Search::recent(pushos_domain::memory::DEFAULT_LIMIT)
                        .within(context.surface.workspace.clone())
                } else {
                    Self::search_from(&context)
                };
                let found = self.notes.find(&search).await.map_err(into_action_error)?;
                Ok(Self::report(&found, search.text.trim()))
            }

            "brief" => {
                let search = Self::search_from(&context);
                let found = self.notes.find(&search).await.map_err(into_action_error)?;
                Ok(self.brief(&found, &context.surface).await)
            }

            "refresh" => {
                let counted = self.notes.refresh().await.map_err(into_action_error)?;
                Ok(ActionResult {
                    status: ActionStatus::Completed,
                    message: Some(format!("{counted} notes")),
                    display: Some(DisplayIntent::Toast {
                        title: format!("{counted} notes"),
                        detail: Some("indexed".to_owned()),
                    }),
                })
            }

            other => Err(ActionError::UnknownVerb {
                provider: self.name(),
                verb: other.to_owned(),
            }),
        }
    }
}

/// Stands in for a runner before one is given, so the weak reference has a type.
#[derive(Debug)]
struct NoActions;

#[async_trait]
impl ActionRunner for NoActions {
    async fn run(
        &self,
        _definition: ActionDefinition,
        _surface: SurfaceContext,
    ) -> Result<ActionResult, ActionError> {
        unreachable!("the placeholder runner is never upgraded")
    }
}

fn invalid(reason: &'static str) -> ActionError {
    ActionError::backend(
        reason,
        ErrorClass::Validation,
        std::io::Error::other(reason),
    )
}

fn into_action_error(error: MemoryError) -> ActionError {
    let class = error.class();
    ActionError::backend(error.to_string(), class, error)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pushos_domain::ids::{ExecutionId, SourceId, WorkspaceId};
    use pushos_domain::ports::{NoteIndex, Source};
    use pushos_memory::{ForgetfulNotes, MarkdownLibrary};
    use pushos_storage::StorageWriter;
    use pushos_testkit::FakeActions;

    use super::*;

    /// A directory that cleans up after itself.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("pushos-memact-{}", ExecutionId::generate()));
            std::fs::create_dir_all(&path).expect("the temporary directory is writable");
            Self(path)
        }

        fn put(&self, name: &str, text: &str) {
            std::fs::write(self.0.join(name), text).expect("writable");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// A provider over real files, a real index and a recording dispatcher.
    struct Rig {
        provider: Arc<MemoryProvider>,
        actions: FakeActions,
        scratch: Scratch,
        _runner: Arc<dyn ActionRunner>,
        _writer: Option<StorageWriter>,
    }

    impl Rig {
        async fn new(brief: Option<&str>) -> Self {
            let scratch = Scratch::new();
            let writer = StorageWriter::in_memory().expect("an in-memory database always opens");
            let index: Arc<dyn NoteIndex> = Arc::new(writer.handle());

            let library = MarkdownLibrary::new(
                vec![Source {
                    id: SourceId::new("notes"),
                    root: scratch.0.clone(),
                    writable: true,
                }],
                index,
            );

            let brief = brief.map(|written| {
                written
                    .parse::<ActionSelector>()
                    .expect("a valid brief selector")
            });
            let provider = Arc::new(MemoryProvider::new(Arc::new(library), brief));

            let actions = FakeActions::new();
            let runner: Arc<dyn ActionRunner> = Arc::new(actions.clone());
            provider.use_actions(&runner).await;

            Self {
                provider,
                actions,
                scratch,
                _runner: runner,
                _writer: Some(writer),
            }
        }

        async fn run(&self, verb: &str, params: Params) -> ActionResult {
            self.provider
                .execute(context(verb, params, None))
                .await
                .expect("the action succeeds")
        }
    }

    fn context(verb: &str, params: Params, workspace: Option<&str>) -> ActionContext {
        let definition = ActionDefinition::new(
            format!("memory.{verb}")
                .parse::<ActionSelector>()
                .expect("a valid selector"),
            params,
        );
        let mut surface = SurfaceContext::empty();
        surface.workspace = workspace.map(WorkspaceId::new);
        ActionContext::new(
            definition,
            pushos_domain::ids::CorrelationId::generate(),
            surface,
        )
    }

    fn saying(text: &str) -> Params {
        let mut params = Params::new();
        params.set(WORDS, ParamValue::Text(text.into()));
        params
    }

    /// The lines an action put on the display.
    fn lines(result: &ActionResult) -> Vec<String> {
        match &result.display {
            Some(DisplayIntent::Report { lines, .. }) => lines.clone(),
            _ => Vec::new(),
        }
    }

    #[tokio::test]
    async fn one_press_writes_down_what_was_said() {
        let rig = Rig::new(None).await;

        let result = rig
            .run("write", saying("The deploy needs the new token"))
            .await;
        assert_eq!(result.status, ActionStatus::Completed);

        let written: Vec<_> = std::fs::read_dir(&rig.scratch.0)
            .expect("readable")
            .flatten()
            .collect();
        assert_eq!(written.len(), 1, "a note should be a file on disk");
    }

    #[tokio::test]
    async fn a_note_written_on_one_press_is_found_by_the_next() {
        let rig = Rig::new(None).await;
        rig.run("write", saying("The deploy needs the new token"))
            .await;

        let found = rig.run("find", saying("token")).await;
        assert_eq!(lines(&found).len(), 1, "{found:?}");
        assert!(lines(&found)[0].contains("deploy"), "{:?}", lines(&found));
    }

    #[tokio::test]
    async fn pressing_the_capture_pad_with_nothing_to_say_is_refused() {
        let rig = Rig::new(None).await;
        let error = rig
            .provider
            .execute(context("write", saying("   "), None))
            .await
            .expect_err("there is nothing to write down");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn a_note_is_filed_under_the_project_the_operator_is_in() {
        let rig = Rig::new(None).await;
        rig.provider
            .execute(context("write", saying("mine"), Some("sydclaw")))
            .await
            .expect("writing succeeds");
        rig.provider
            .execute(context("write", saying("theirs"), Some("other")))
            .await
            .expect("writing succeeds");

        let found = rig
            .provider
            .execute(context("recent", Params::new(), Some("sydclaw")))
            .await
            .expect("searching succeeds");
        assert_eq!(lines(&found).len(), 1, "{:?}", lines(&found));
        assert!(lines(&found)[0].contains("mine"));
    }

    #[tokio::test]
    async fn a_search_can_be_widened_past_the_current_project() {
        let rig = Rig::new(None).await;
        rig.provider
            .execute(context("write", saying("elsewhere"), Some("other")))
            .await
            .expect("writing succeeds");

        let mut params = saying("elsewhere");
        params.set("workspace", ParamValue::Text("all".into()));
        let found = rig
            .provider
            .execute(context("find", params, Some("sydclaw")))
            .await
            .expect("searching succeeds");
        assert_eq!(lines(&found).len(), 1);
    }

    #[tokio::test]
    async fn finding_nothing_says_so_rather_than_showing_an_empty_list() {
        let rig = Rig::new(None).await;
        let found = rig.run("find", saying("nothing like this exists")).await;

        assert!(lines(&found).is_empty());
        assert!(
            matches!(found.display, Some(DisplayIntent::Toast { .. })),
            "an empty panel would look like a fault"
        );
    }

    #[tokio::test]
    async fn what_was_found_can_be_handed_to_an_agent() {
        let rig = Rig::new(Some("agent.prompt")).await;
        rig.run("write", saying("The deploy needs the new token"))
            .await;

        let result = rig.run("brief", saying("token")).await;
        assert_eq!(result.status, ActionStatus::Completed);

        let ran = rig.actions.ran();
        assert_eq!(ran.len(), 1);
        assert_eq!(ran[0].selector.to_string(), "agent.prompt");
        assert!(
            ran[0]
                .params
                .text(WORDS)
                .is_some_and(|text| text.contains("new token")),
            "the agent has to receive the note, not a reference to it"
        );
    }

    #[tokio::test]
    async fn a_briefing_with_nowhere_to_go_shows_what_it_found() {
        let rig = Rig::new(None).await;
        rig.run("write", saying("The deploy needs the new token"))
            .await;

        let result = rig.run("brief", saying("token")).await;
        assert!(rig.actions.ran().is_empty(), "nothing was configured");
        assert_eq!(lines(&result).len(), 1, "so showing them is what is left");
    }

    #[tokio::test]
    async fn a_briefing_that_found_nothing_sends_nothing() {
        // An agent handed an empty briefing has been interrupted for nothing.
        let rig = Rig::new(Some("agent.prompt")).await;
        rig.run("brief", saying("nothing like this exists")).await;
        assert!(rig.actions.ran().is_empty());
    }

    #[tokio::test]
    async fn notes_written_by_hand_are_picked_up_when_asked() {
        let rig = Rig::new(None).await;
        rig.scratch
            .put("decision.md", "# One write owner\n\nthe storage task\n");

        let refreshed = rig.run("refresh", Params::new()).await;
        assert_eq!(refreshed.message.as_deref(), Some("1 notes"));
        assert_eq!(lines(&rig.run("find", saying("storage")).await).len(), 1);
    }

    #[tokio::test]
    async fn only_the_verb_that_writes_a_file_needs_permission() {
        let capabilities = Rig::new(None).await.provider.capabilities();
        assert_eq!(
            capabilities.required_for(&ActionVerb::new("write")),
            [Permission::FilesystemWrite]
        );
        for reading in ["find", "recent", "brief", "refresh"] {
            assert!(
                capabilities
                    .required_for(&ActionVerb::new(reading))
                    .is_empty(),
                "`{reading}` reads directories the operator already configured"
            );
        }
    }

    #[tokio::test]
    async fn an_unknown_verb_is_refused_by_name() {
        let rig = Rig::new(None).await;
        let error = rig
            .provider
            .execute(context("summarise", Params::new(), None))
            .await
            .expect_err("there is no such verb");
        assert!(matches!(error, ActionError::UnknownVerb { .. }));
    }

    #[tokio::test]
    async fn search_works_with_no_database_behind_it() {
        // Memory is optional, and so is the index under it.
        let scratch = Scratch::new();
        scratch.put("a.md", "# Deploy\n\nthe new token\n");
        let library = MarkdownLibrary::new(
            vec![Source {
                id: SourceId::new("notes"),
                root: scratch.0.clone(),
                writable: true,
            }],
            Arc::new(ForgetfulNotes),
        );
        let provider = MemoryProvider::new(Arc::new(library), None);

        let found = provider
            .execute(context("find", saying("token"), None))
            .await
            .expect("searching succeeds");
        assert_eq!(lines(&found).len(), 1);
    }
}
