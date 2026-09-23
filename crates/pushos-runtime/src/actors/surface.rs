//! The surface's live state.
//!
//! One component owns where the operator is and what the display is saying.
//! Everything else reads an immutable snapshot, so there is no shared mutable
//! state and no lock on the input path.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pushos_config::RuntimeConfig;
use pushos_domain::action::DisplayIntent;
use pushos_domain::context::SurfaceContext;
use pushos_domain::ids::{PageId, WorkspaceId};
use pushos_domain::page::PageTarget;
use pushos_ui::{Notice, Overlay, PageView, Splash, SurfacePresence, Tone, UiSnapshot};

/// How long a transient message stays on screen.
pub(crate) const NOTICE_LIFETIME: Duration = Duration::from_secs(4);
/// How long an overlay stays before the previous view returns.
pub(crate) const OVERLAY_LIFETIME: Duration = Duration::from_secs(6);
/// How long the waiting screen stays when the surface first appears.
///
/// Long enough to see, short enough that it never stands between the operator
/// and their pads.
pub(crate) const SPLASH_LIFETIME: Duration = Duration::from_millis(2_600);

/// Where the operator is, and what the display is currently saying.
#[derive(Debug)]
pub struct SurfaceState {
    config: Arc<RuntimeConfig>,
    page: Option<PageId>,
    previous_page: Option<PageId>,
    workspace: Option<WorkspaceId>,
    surface: SurfacePresence,
    notice: Option<Timed<Notice>>,
    overlay: Option<Timed<Overlay>>,
    /// One thing being looked at closely.
    ///
    /// Untimed, unlike everything else here: an operator reading what a
    /// session is doing is reading, and a panel that reverted underneath them
    /// would be one they could not use. It goes when they move, or when they
    /// look at something else.
    focus: Option<pushos_ui::Focus>,
    /// Where the operator is in the browser, while it is open.
    ///
    /// Only the cursor. What is at that place is built from live state when
    /// the display is drawn, so a session that ends while the browser is open
    /// cannot leave it pointing at something that is no longer there.
    browsing: Option<pushos_domain::browse::Browsing>,
    splash_until: Option<Instant>,
    /// What the sessions are doing, as the display should show it.
    ///
    /// Kept here rather than read from the supervisor when drawing: the
    /// renderer takes an immutable snapshot and must never wait on a lock.
    sessions: Vec<pushos_ui::SessionLine>,
    /// Whether the microphone is on.
    listening: pushos_domain::voice::Listening,
    /// What the browser chose, waiting to be run.
    chosen: Option<pushos_domain::action::ActionDefinition>,
}

/// What an operator calls a control.
///
/// The name printed on the hardware rather than the one written in
/// configuration: nobody looking for a way out reads `button.session`, they
/// look for the button that says Session.
fn name_of(control: pushos_domain::controls::ControlId) -> String {
    control
        .to_string()
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .replace('_', " ")
        .to_uppercase()
}

/// How a status reads on the display.
///
/// The one place the two vocabularies meet. The domain says what a thing is;
/// the display says how loud that should be, and whether anything moves.
const fn tone_of(status: pushos_domain::color::StatusColor) -> Tone {
    use pushos_domain::color::StatusColor;
    match status {
        // Only what is genuinely going anywhere. Something merely part of a
        // workflow, or with a line typed and not sent, is not working, and a
        // panel that animated for it would be animating for nothing.
        StatusColor::Working => Tone::Active,
        StatusColor::Waiting => Tone::Attention,
        StatusColor::Failed => Tone::Failure,
        StatusColor::Idle | StatusColor::Unassigned => Tone::Muted,
        StatusColor::Complete | StatusColor::Selected | StatusColor::Workflow => Tone::Normal,
    }
}

/// Something that goes away on its own.
#[derive(Debug)]
struct Timed<T> {
    value: T,
    expires_at: Instant,
}

impl SurfaceState {
    /// Builds the state for a configuration, starting on its home page.
    pub fn new(config: Arc<RuntimeConfig>) -> Self {
        let page = config
            .home_page
            .clone()
            .or_else(|| config.pages.first().map(|p| p.id.clone()));
        Self {
            config,
            page,
            previous_page: None,
            workspace: None,
            surface: SurfacePresence::Absent,
            notice: None,
            overlay: None,
            focus: None,
            browsing: None,
            splash_until: None,
            sessions: Vec::new(),
            listening: pushos_domain::voice::Listening::Idle,
            chosen: None,
        }
    }

    /// Records what the sessions are doing.
    pub fn set_sessions(&mut self, sessions: Vec<pushos_ui::SessionLine>) {
        self.sessions = sessions;
    }

    /// The control that takes the operator back to where they were.
    ///
    /// Found in the bindings rather than written down twice: whatever the
    /// operator bound to going home is what the panel should name, and a hint
    /// that named a control they had rebound would be worse than none.
    fn way_back(&self) -> Option<String> {
        use pushos_domain::gesture::Gesture;

        let context = self.context(false);
        self.config
            .bindings
            .iter()
            .filter(|binding| binding.scope.applies_to(&context))
            .filter(|binding| matches!(binding.gesture, Gesture::Press | Gesture::Tap))
            .find(|binding| {
                let selector = &binding.action.selector;
                selector.provider.as_str() == "page"
                    && matches!(selector.verb.as_str(), "home" | "back" | "show")
            })
            .map(|binding| name_of(binding.control))
    }

    /// Records whether the microphone is on.
    pub fn set_listening(&mut self, listening: pushos_domain::voice::Listening) {
        self.listening = listening;
    }

    /// Adopts a new configuration.
    ///
    /// The current page is kept when it still exists, so a reload does not move
    /// the operator out from under their own hands.
    pub fn adopt(&mut self, config: Arc<RuntimeConfig>) {
        let keep_current = self
            .page
            .as_ref()
            .is_some_and(|page| config.pages.iter().any(|candidate| candidate.id == *page));

        if !keep_current {
            self.page = config
                .home_page
                .clone()
                .or_else(|| config.pages.first().map(|p| p.id.clone()));
            self.previous_page = None;
        }
        self.config = config;
    }

    /// The configuration currently in force.
    pub fn config(&self) -> &Arc<RuntimeConfig> {
        &self.config
    }

    /// The context binding resolution runs against.
    pub fn context(&self, shift_held: bool) -> SurfaceContext {
        SurfaceContext {
            page: self.page.clone(),
            workspace: self.workspace.clone(),
            session: None,
            shift_held,
        }
    }

    /// Records what the display is attached to.
    ///
    /// A surface arriving raises the waiting screen for a moment, which is the
    /// one time the display has nothing more useful to say.
    pub fn set_surface(&mut self, surface: SurfacePresence, now: Instant) {
        let arriving = surface != SurfacePresence::Absent;
        if arriving && self.surface == SurfacePresence::Absent {
            self.splash_until = Some(now + SPLASH_LIFETIME);
        }
        if !arriving {
            self.splash_until = None;
        }
        self.surface = surface;
    }

    /// What the display is attached to.
    pub const fn surface(&self) -> SurfacePresence {
        self.surface
    }

    /// The workspace in effect.
    pub fn workspace(&self) -> Option<WorkspaceId> {
        self.workspace.clone()
    }

    /// Moves to a workspace.
    pub fn select_workspace(&mut self, workspace: Option<WorkspaceId>) {
        self.workspace = workspace;
    }

    /// The page currently in effect.
    pub fn page(&self) -> Option<&PageId> {
        self.page.as_ref()
    }

    /// Everything the browser is built from.
    fn rooted(&self) -> crate::actors::tree::Rooted<'_> {
        crate::actors::tree::Rooted {
            config: &self.config,
            sessions: &self.sessions,
        }
    }

    /// What the browser should show, when it is open.
    fn browser(&self) -> Option<pushos_ui::Browser> {
        let browsing = self.browsing.as_ref()?;
        let tree = self.rooted();
        let (rows, empty) = tree.level(browsing.path());
        Some(pushos_ui::Browser {
            trail: crate::actors::tree::Rooted::trail(browsing.path()),
            at: browsing.at(rows.len()),
            rows,
            empty,
        })
    }

    /// Moves the browser, and returns what choosing a row asked for.
    ///
    /// Opening it does not disturb what is underneath: a focus stays where it
    /// was and comes back when the browser closes.
    fn browse(&mut self, move_to: pushos_domain::browse::BrowseMove) {
        use pushos_domain::browse::{BrowseMove, Browsing};

        match move_to {
            BrowseMove::Open => {
                self.browsing.get_or_insert_with(Browsing::opened);
            }
            BrowseMove::Close => self.browsing = None,
            BrowseMove::Step(by) => {
                let rows = self.rows_here();
                if let Some(browsing) = self.browsing.as_mut() {
                    browsing.step(by, rows);
                }
            }
            BrowseMove::Enter => self.enter(),
            BrowseMove::Leave => {
                // Leaving the top closes the browser, which is how a single
                // control both goes back and gets out.
                if self
                    .browsing
                    .as_mut()
                    .is_some_and(|browsing| !browsing.ascend())
                {
                    self.browsing = None;
                }
            }
        }
    }

    /// Goes into the row under the cursor, if it leads anywhere.
    fn enter(&mut self) {
        let rows = self.rows_here();
        let Some(browsing) = self.browsing.as_ref() else {
            return;
        };
        let (here, _) = self.rooted().level(browsing.path());
        let at = browsing.at(rows);
        let Some(twig) = here.get(at) else {
            return;
        };

        if twig.has_children {
            if let Some(browsing) = self.browsing.as_mut() {
                browsing.descend(rows);
            }
        } else {
            // A leaf is a thing to do, and doing it is the end of browsing:
            // the operator came here to get somewhere and has got there.
            self.chosen = self.rooted().choice(browsing.path(), at);
            self.browsing = None;
        }
    }

    /// How many rows are at the cursor's level.
    fn rows_here(&self) -> usize {
        self.browsing
            .as_ref()
            .map_or(0, |browsing| self.rooted().level(browsing.path()).0.len())
    }

    /// Takes what the browser chose, if anything.
    ///
    /// The surface cannot run an action itself, so it holds what was chosen
    /// until whatever owns the dispatcher comes to collect it.
    pub fn taken(&mut self) -> Option<pushos_domain::action::ActionDefinition> {
        self.chosen.take()
    }

    /// Applies an action's display instruction.
    ///
    /// Returns the page moved to, when the instruction moved to one.
    pub fn apply(&mut self, intent: DisplayIntent, now: Instant) -> Option<PageId> {
        match intent {
            DisplayIntent::Page(target) => {
                // Moving is looking away. Whatever was being read closely was
                // about where the operator was, not where they are going.
                self.focus = None;
                self.go_to(&target)
            }
            DisplayIntent::Overview => {
                self.focus = None;
                None
            }

            // The browser is a cursor and nothing else here: what is under it
            // is worked out when the display is drawn. Moving it never touches
            // the focus underneath, which is waiting for the browser to close.
            DisplayIntent::Browse(move_to) => {
                self.browse(move_to);
                None
            }
            DisplayIntent::Focus {
                kind,
                title,
                state,
                tone,
                lines,
                depth,
            } => {
                // The others come from what the surface already knows, so a
                // provider never has to describe anything but its own thing.
                let others: Vec<pushos_ui::SessionLine> = self
                    .sessions
                    .iter()
                    .filter(|line| line.name != title)
                    .cloned()
                    .collect();

                let mut view = pushos_ui::Focus::new(kind, title)
                    .doing(state, tone_of(tone))
                    .saying(lines)
                    .at_depth(depth)
                    .beside(others);
                if let Some(control) = self.way_back() {
                    view = view.leaving_with(control);
                }
                self.focus = Some(view);
                // An overlay on top of something being read closely would hide
                // the thing that was asked for.
                self.overlay = None;
                None
            }
            DisplayIntent::Toast { title, detail } => {
                self.overlay = Some(Timed {
                    value: {
                        let overlay = Overlay::new("", title);
                        match detail {
                            Some(detail) => overlay.with_detail(detail),
                            None => overlay,
                        }
                    },
                    expires_at: now + OVERLAY_LIFETIME,
                });
                None
            }
            DisplayIntent::Report { kind, title, lines } => {
                self.overlay = Some(Timed {
                    value: Overlay::new(kind, title).listing(lines),
                    expires_at: now + OVERLAY_LIFETIME,
                });
                None
            }
            DisplayIntent::Prompt { question, choices } => {
                // A prompt does not steal the surface: it takes the display,
                // but the operator's page and bindings are untouched underneath.
                self.overlay = Some(Timed {
                    value: Overlay::new("Question", question).asking(choices),
                    expires_at: now + OVERLAY_LIFETIME,
                });
                None
            }
        }
    }

    /// Shows a transient message.
    pub fn notify(&mut self, text: impl Into<String>, tone: Tone, now: Instant) {
        self.notice = Some(Timed {
            value: Notice::new(text, tone),
            expires_at: now + NOTICE_LIFETIME,
        });
    }

    /// Clears anything that has outlived its welcome.
    ///
    /// Returns whether anything went away, so the caller knows a redraw is due.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = (
            self.notice.is_some(),
            self.overlay.is_some(),
            self.splash_until.is_some(),
        );

        self.notice.take_if(|notice| now >= notice.expires_at);
        self.overlay.take_if(|overlay| now >= overlay.expires_at);
        self.splash_until.take_if(|until| now >= *until);

        before
            != (
                self.notice.is_some(),
                self.overlay.is_some(),
                self.splash_until.is_some(),
            )
    }

    /// When the next thing expires, if anything will.
    pub fn next_expiry(&self) -> Option<Instant> {
        [
            self.notice.as_ref().map(|timed| timed.expires_at),
            self.overlay.as_ref().map(|timed| timed.expires_at),
            self.splash_until,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Builds the display's view of the current state.
    pub fn snapshot(&self) -> UiSnapshot {
        let page = self.page.as_ref().and_then(|id| {
            self.config
                .pages
                .iter()
                .find(|candidate| candidate.id == *id)
        });

        let view = page.map_or_else(PageView::default, |page| {
            let mut view = PageView::new(page.id.clone(), page.name.clone());
            if let Some((index, total)) = self.config.page_position(&page.id) {
                view = view.at(index, total);
            }
            view
        });

        UiSnapshot {
            page: view,
            workspace: self.workspace.as_ref().map(ToString::to_string),
            surface: self.surface,
            slots: crate::actors::slots::labels_for(&self.config, &self.context(false)),
            footer: page.and_then(|page| page.description.clone()),
            notice: self.notice.as_ref().map(|timed| timed.value.clone()),
            overlay: self.overlay.as_ref().map(|timed| timed.value.clone()),
            browser: self.browser(),
            focus: self.focus.clone(),
            // The frame is left at zero: the renderer owns the animation, so a
            // snapshot never has to be republished just because time passed.
            sessions: self.sessions.clone(),
            listening: self.listening,
            // Stamped by the renderer, which is the only thing that knows what
            // time it is when a frame is drawn.
            frame: 0,
            splash: self.splash_until.map(|_| {
                Splash::new("PushOS", 0).with_detail(match &self.workspace {
                    Some(workspace) => workspace.to_string(),
                    None => "ready".to_owned(),
                })
            }),
        }
    }

    fn go_to(&mut self, target: &PageTarget) -> Option<PageId> {
        let next = match target {
            PageTarget::Named(page) => self
                .config
                .pages
                .iter()
                .find(|candidate| candidate.id == *page)
                .map(|page| page.id.clone()),
            PageTarget::Next => self
                .config
                .page_after(self.page.as_ref())
                .map(|p| p.id.clone()),
            PageTarget::Previous => self
                .config
                .page_before(self.page.as_ref())
                .map(|p| p.id.clone()),
            PageTarget::Home => self.config.home_page.clone(),
            PageTarget::Back => self.previous_page.clone(),
        }?;

        if Some(&next) == self.page.as_ref() {
            return None;
        }

        self.previous_page = self.page.replace(next.clone());
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use pushos_config::ConfigFile;
    use pushos_domain::action::DisplayIntent;
    use pushos_domain::browse::BrowseMove;

    use super::*;

    const SOME: &str = r#"
        [[pages]]
        id = "home"
        name = "Home"

        [[pages]]
        id = "music"
        name = "Music"

        [[workspaces]]
        id = "pushos"
        name = "PushOS"
        root = "/tmp/pushos"

        [[workspaces]]
        id = "other"
        name = "Other"
        root = "/tmp/other"
    "#;

    fn surface() -> SurfaceState {
        let parsed: ConfigFile = toml::from_str(SOME).expect("well-formed");
        let config = RuntimeConfig::build(&parsed).expect("valid");
        SurfaceState::new(Arc::new(config))
    }

    fn browse(state: &mut SurfaceState, move_to: BrowseMove) {
        state.apply(DisplayIntent::Browse(move_to), Instant::now());
    }

    fn showing(state: &SurfaceState) -> pushos_ui::Browser {
        state.snapshot().browser.expect("the browser is open")
    }

    #[test]
    fn the_browser_is_shut_until_it_is_opened() {
        let mut state = surface();
        assert!(state.snapshot().browser.is_none());

        browse(&mut state, BrowseMove::Open);
        assert!(state.snapshot().browser.is_some());

        browse(&mut state, BrowseMove::Close);
        assert!(state.snapshot().browser.is_none());
    }

    #[test]
    fn opening_the_browser_leaves_what_is_being_read_alone() {
        // The one thing that must not change: a focus is untouched by the
        // browser opening over it, and is still there when it closes.
        let mut state = surface();
        state.apply(
            DisplayIntent::Focus {
                kind: "session".to_owned(),
                title: "Claude Code".to_owned(),
                state: "working".to_owned(),
                tone: pushos_domain::color::StatusColor::Working,
                lines: vec!["running the tests".to_owned()],
                depth: None,
            },
            Instant::now(),
        );
        let reading = state.snapshot().focus.expect("something is being read");

        browse(&mut state, BrowseMove::Open);
        browse(&mut state, BrowseMove::Step(1));
        assert_eq!(
            state.snapshot().focus.as_ref(),
            Some(&reading),
            "browsing must not disturb what is being read"
        );

        browse(&mut state, BrowseMove::Close);
        assert_eq!(
            state.snapshot().focus.as_ref(),
            Some(&reading),
            "and it should still be there afterwards"
        );
    }

    #[test]
    fn the_cursor_walks_the_level_and_stops_at_the_ends() {
        let mut state = surface();
        browse(&mut state, BrowseMove::Open);
        assert_eq!(showing(&state).at, 0);

        browse(&mut state, BrowseMove::Step(1));
        assert_eq!(showing(&state).at, 1);

        for _ in 0..10 {
            browse(&mut state, BrowseMove::Step(1));
        }
        let rows = showing(&state).rows.len();
        assert_eq!(showing(&state).at, rows - 1, "it stops at the bottom");
    }

    #[test]
    fn going_into_projects_lists_them() {
        let mut state = surface();
        browse(&mut state, BrowseMove::Open);
        browse(&mut state, BrowseMove::Enter);

        let shown = showing(&state);
        assert_eq!(shown.trail, "Browse › Projects");
        assert_eq!(shown.rows.len(), 2);
        assert_eq!(shown.rows[0].label, "PushOS");
        assert!(!shown.rows[0].has_children, "a project is a leaf");
    }

    #[test]
    fn choosing_a_project_runs_it_and_closes_the_browser() {
        let mut state = surface();
        browse(&mut state, BrowseMove::Open);
        browse(&mut state, BrowseMove::Enter);
        browse(&mut state, BrowseMove::Step(1));
        browse(&mut state, BrowseMove::Enter);

        let chosen = state.taken().expect("choosing a leaf asks for an action");
        assert_eq!(chosen.selector.to_string(), "workspace.select");
        assert_eq!(chosen.params.text("target"), Some("other"));
        assert!(
            state.snapshot().browser.is_none(),
            "the operator came here to get somewhere and has got there"
        );
        assert!(state.taken().is_none(), "and it is only taken once");
    }

    #[test]
    fn leaving_goes_back_out_and_then_closes() {
        let mut state = surface();
        browse(&mut state, BrowseMove::Open);
        browse(&mut state, BrowseMove::Step(2));
        browse(&mut state, BrowseMove::Enter);
        assert_eq!(showing(&state).trail, "Browse › Pages");

        browse(&mut state, BrowseMove::Leave);
        assert_eq!(showing(&state).trail, "Browse");
        assert_eq!(showing(&state).at, 2, "back on the row it went in by");

        // Leaving the top is how one control both goes back and gets out.
        browse(&mut state, BrowseMove::Leave);
        assert!(state.snapshot().browser.is_none());
    }

    #[test]
    fn moving_the_cursor_chooses_nothing() {
        let mut state = surface();
        browse(&mut state, BrowseMove::Open);
        for move_to in [
            BrowseMove::Step(1),
            BrowseMove::Step(-1),
            BrowseMove::Leave,
            BrowseMove::Open,
        ] {
            browse(&mut state, move_to);
            assert!(state.taken().is_none(), "{move_to:?} should run nothing");
        }
    }

    #[test]
    fn opening_an_already_open_browser_keeps_the_operators_place() {
        let mut state = surface();
        browse(&mut state, BrowseMove::Open);
        browse(&mut state, BrowseMove::Step(2));
        browse(&mut state, BrowseMove::Open);
        assert_eq!(showing(&state).at, 2, "a second press is not a reset");
    }
}
