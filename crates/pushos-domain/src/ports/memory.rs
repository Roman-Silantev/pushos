//! The memory ports.
//!
//! Two, kept apart because they answer to different owners. A store owns the
//! notes themselves, which on this machine are files the operator can read
//! without PushOS. An index owns finding them quickly, which is PushOS's own
//! bookkeeping and can be thrown away and rebuilt.
//!
//! That split is what makes memory optional. Without an index, search falls
//! back to reading; without a store, there is nothing to search and the
//! namespace is simply not offered.

use async_trait::async_trait;

use crate::ids::{NoteId, SourceId};
use crate::memory::{Draft, Excerpt, Note, Search};

/// Where notes live.
#[async_trait]
pub trait MemoryStore: Send + Sync + std::fmt::Debug {
    /// Writes a note down and hands back what was written.
    async fn write(&self, draft: &Draft) -> Result<Note, MemoryError>;

    /// Finds notes worth showing.
    async fn find(&self, search: &Search) -> Result<Vec<Excerpt>, MemoryError>;

    /// Reads one note in full.
    ///
    /// `None` rather than an error when it is not there: a note the operator
    /// deleted from their own directory is a fact, not a fault.
    async fn read(&self, note: &NoteId) -> Result<Option<Note>, MemoryError>;

    /// Reads what is on disk and brings the index up to date.
    ///
    /// Reports how many notes there are afterwards. Notes are files, and files
    /// change without PushOS being told.
    async fn refresh(&self) -> Result<usize, MemoryError>;

    /// Where notes are kept, in the order they are searched.
    fn sources(&self) -> Vec<Source>;
}

/// One place notes are kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// What it is called.
    pub id: SourceId,
    /// Where it is on disk.
    pub root: std::path::PathBuf,
    /// Whether PushOS may add to it.
    ///
    /// A directory the operator shares with something else is worth reading and
    /// not worth writing to, and saying so beats finding out afterwards.
    pub writable: bool,
}

/// Where finding notes quickly is kept.
///
/// Everything here is derived: an index that was deleted is rebuilt by reading
/// the notes again, and one that disagrees with the files is wrong rather than
/// authoritative.
#[async_trait]
pub trait NoteIndex: Send + Sync + std::fmt::Debug {
    /// Whether this index keeps anything.
    ///
    /// One that does not is a real answer rather than a broken one: PushOS runs
    /// without a database and reads the notes instead. Asked rather than
    /// inferred from an empty result, because a search that genuinely matches
    /// nothing must not send the caller off to read every file.
    fn keeps(&self) -> bool {
        true
    }

    /// Records a note, replacing whatever was there under the same identity.
    async fn record(&self, note: &Note) -> Result<(), MemoryError>;

    /// Finds notes matching a search, best first.
    async fn search(&self, search: &Search) -> Result<Vec<Excerpt>, MemoryError>;

    /// Removes one note.
    async fn erase(&self, note: &NoteId) -> Result<(), MemoryError>;

    /// Removes everything indexed from one source, before it is read again.
    async fn empty(&self, source: &SourceId) -> Result<(), MemoryError>;
}

/// Why a note could not be written, found or read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MemoryError {
    /// There is nowhere PushOS may write.
    ///
    /// Its own kind because the fix is a line of configuration rather than
    /// anything about the note.
    #[error("{context}")]
    NowhereToWrite {
        /// What is missing, and what would fix it.
        context: String,
    },

    /// A note has nothing in it.
    #[error("there is nothing to write down")]
    Empty,

    /// The notes themselves could not be reached.
    #[error("{context}")]
    Unreadable {
        /// What PushOS was attempting.
        context: String,
        /// The originating fault.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl MemoryError {
    /// How a caller should react.
    pub const fn class(&self) -> crate::error::ErrorClass {
        use crate::error::ErrorClass;
        match self {
            // Both are the operator's to fix, and both messages say how.
            Self::NowhereToWrite { .. } | Self::Empty => ErrorClass::Validation,
            Self::Unreadable { .. } => ErrorClass::Retryable,
        }
    }

    /// Reports that the notes could not be reached.
    pub fn unreadable(
        context: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Unreadable {
            context: context.into(),
            source: Box::new(source),
        }
    }
}
