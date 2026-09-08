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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_keeps_the_id_it_was_given() {
        let page = Page::new("development", "Development");
        assert_eq!(page.id.as_str(), "development");
        assert_eq!(page.name, "Development");
    }
}
