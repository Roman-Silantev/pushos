//! Pages: named sets of control meanings.

use serde::{Deserialize, Serialize};

use crate::ids::PageId;

/// A named layout that changes what the surface's controls mean.
///
/// A page holds no bindings of its own. Bindings reference a page by id, which
/// keeps a page cheap to switch to and impossible to desynchronise.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    /// Stable identity used by bindings and actions.
    pub id: PageId,
    /// Name shown on the display.
    pub name: String,
    /// Optional longer description shown in Studio.
    pub description: Option<String>,
}

impl Page {
    /// Builds a page.
    pub fn new(id: impl Into<PageId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
        }
    }

    /// Attaches a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Which page an action wants to move to.
///
/// Relative movement is expressed here rather than resolved by the provider,
/// because only the component that owns the current page knows what "next"
/// means. Keeping the provider free of that state is what lets page actions be
/// tested without a running surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageTarget {
    /// A specific page.
    Named(PageId),
    /// The next page in configured order, wrapping at the end.
    Next,
    /// The previous page in configured order, wrapping at the start.
    Previous,
    /// The configured home page.
    Home,
    /// Wherever the operator was before the current page.
    Back,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_keeps_the_id_it_was_given() {
        let page = Page::new("development", "Development");
        assert_eq!(page.id.as_str(), "development");
        assert_eq!(page.name, "Development");
    }

    #[test]
    fn a_named_target_carries_the_page_it_names() {
        let target = PageTarget::Named("music".into());
        assert_eq!(target, PageTarget::Named(PageId::new("music")));
        assert_ne!(target, PageTarget::Home);
    }
}
