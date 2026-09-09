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
    splash_until: Option<Instant>,
    /// What the sessions are doing, as the display should show it.
    ///
    /// Kept here rather than read from the supervisor when drawing: the
    /// renderer takes an immutable snapshot and must never wait on a lock.
    sessions: Vec<pushos_ui::SessionLine>,
    /// Whether the microphone is on.
    listening: pushos_domain::voice::Listening,
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
            splash_until: None,
            sessions: Vec::new(),
            listening: pushos_domain::voice::Listening::Idle,
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
            DisplayIntent::Focus {
                kind,
                title,
                state,
                tone,
                lines,
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
