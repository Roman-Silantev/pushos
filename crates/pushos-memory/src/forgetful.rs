//! An index that remembers nothing.
//!
//! What PushOS uses when there is no database: notes are still written, still
//! read and still listed, they simply are not searched quickly. Memory is meant
//! to be optional, and "optional" has to mean the feature degrades rather than
//! disappears.

use async_trait::async_trait;
use pushos_domain::ids::{NoteId, SourceId};
use pushos_domain::memory::{Excerpt, Note, Search};
use pushos_domain::ports::{MemoryError, NoteIndex};

/// An index that keeps nothing between calls.
#[derive(Debug, Default)]
pub struct ForgetfulNotes;

#[async_trait]
impl NoteIndex for ForgetfulNotes {
    /// Says so, so the library reads the notes rather than believing this.
    fn keeps(&self) -> bool {
        false
    }

    async fn record(&self, _note: &Note) -> Result<(), MemoryError> {
        Ok(())
    }

    /// Finds nothing, which is honest.
    ///
    /// The library reads the files itself when an index does not keep anything,
    /// so this is slower rather than useless.
    async fn search(&self, _search: &Search) -> Result<Vec<Excerpt>, MemoryError> {
        Ok(Vec::new())
    }

    async fn erase(&self, _note: &NoteId) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn empty(&self, _source: &SourceId) -> Result<(), MemoryError> {
        Ok(())
    }
}
