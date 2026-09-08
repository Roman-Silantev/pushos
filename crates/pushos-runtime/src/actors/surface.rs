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
use pushos_ui::{Notice, Overlay, PageView, Tone, UiSnapshot};

/// How long a transient message stays on screen.
pub(crate) const NOTICE_LIFETIME: Duration = Duration::from_secs(4);
/// How long an overlay stays before the previous view returns.
pub(crate) const OVERLAY_LIFETIME: Duration = Duration::from_secs(6);

/// Where the operator is, and what the display is currently saying.
#[derive(Debug)]
pub struct SurfaceState {
    config: Arc<RuntimeConfig>,
    page: Option<PageId>,
    previous_page: Option<PageId>,
    workspace: Option<WorkspaceId>,
    connected: bool,
    notice: Option<Timed<Notice>>,
    overlay: Option<Timed<Overlay>>,
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
            connected: false,
            notice: None,
            overlay: None,
        }
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

    /// Records whether the surface is attached.
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
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
            DisplayIntent::Page(target) => self.go_to(&target),
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
        let had_notice = self.notice.is_some();
        let had_overlay = self.overlay.is_some();

        self.notice.take_if(|notice| now >= notice.expires_at);
        self.overlay.take_if(|overlay| now >= overlay.expires_at);

        had_notice != self.notice.is_some() || had_overlay != self.overlay.is_some()
    }

    /// When the next thing expires, if anything will.
    pub fn next_expiry(&self) -> Option<Instant> {
        let notice = self.notice.as_ref().map(|timed| timed.expires_at);
        let overlay = self.overlay.as_ref().map(|timed| timed.expires_at);
        match (notice, overlay) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (found, None) | (None, found) => found,
        }
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
            connected: self.connected,
            slots: crate::actors::slots::labels_for(&self.config, &self.context(false)),
            footer: page.and_then(|page| page.description.clone()),
            notice: self.notice.as_ref().map(|timed| timed.value.clone()),
            overlay: self.overlay.as_ref().map(|timed| timed.value.clone()),
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
