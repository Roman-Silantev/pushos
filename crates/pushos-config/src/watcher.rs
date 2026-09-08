//! Watching configuration for changes.
//!
//! Editors write files in bursts, and some write a file several times to save
//! it once. Events are therefore debounced, and a reload that fails is a
//! non-event: the operator is still typing, and the running surface stays up.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::store::ConfigStore;

/// How long to wait for a burst of file events to settle.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// How many pending change notifications to hold before coalescing.
///
/// One is enough: the only question a notification answers is "something
/// changed", and the reload re-reads everything anyway.
const CHANNEL_CAPACITY: usize = 1;

/// Watches a configuration root and reloads it when it changes.
///
/// Dropping the watcher stops it.
#[derive(Debug)]
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    changes: mpsc::Receiver<()>,
    store: Arc<ConfigStore>,
}

impl ConfigWatcher {
    /// Starts watching the store's root.
    pub fn start(store: Arc<ConfigStore>) -> Result<Self, WatchError> {
        let (changes_tx, changes) = mpsc::channel(CHANNEL_CAPACITY);
        let root = store.root().to_path_buf();

        let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            match event {
                Ok(event) if is_content_change(event.kind) => {
                    // A full channel already carries "something changed", which
                    // is the entire content of a second notification.
                    let _ = changes_tx.try_send(());
                }
                Ok(_) => {}
                Err(error) => warn!(%error, "configuration watch reported an error"),
            }
        })
        .map_err(|source| WatchError {
            root: root.clone(),
            source,
        })?;

        // Watching the parent of a single file catches editors that replace the
        // file rather than writing into it.
        let target = if root.is_file() {
            root.parent()
                .map_or_else(|| root.clone(), std::path::Path::to_path_buf)
        } else {
            root.clone()
        };

        watcher
            .watch(&target, RecursiveMode::Recursive)
            .map_err(|source| WatchError {
                root: target,
                source,
            })?;

        Ok(Self {
            _watcher: watcher,
            changes,
            store,
        })
    }

    /// Waits for the next settled change and reloads.
    ///
    /// Returns whether the reload succeeded, or `None` once the watcher has
    /// stopped. A failed reload is reported rather than propagated, because the
    /// running configuration is still in force either way.
    pub async fn next_reload(&mut self) -> Option<bool> {
        self.changes.recv().await?;

        // Let the burst finish before re-reading, so a file saved in three
        // writes is read once, and never half-written.
        loop {
            match tokio::time::timeout(DEBOUNCE, self.changes.recv()).await {
                Ok(Some(())) => debug!("configuration still changing; waiting"),
                Ok(None) => return None,
                Err(_) => break,
            }
        }

        Some(self.store.reload_or_keep())
    }
}

/// Whether an event means the configuration's content may have changed.
///
/// Access events fire when a file is merely read, which is not a reason to
/// re-read it.
fn is_content_change(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// The configuration root could not be watched.
#[derive(Debug, thiserror::Error)]
#[error("could not watch `{root}` for changes")]
pub struct WatchError {
    /// The path that could not be watched.
    pub root: PathBuf,
    /// What the file system watcher reported.
    #[source]
    pub source: notify::Error,
}

#[cfg(test)]
mod tests {
    use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind};

    use super::*;

    #[test]
    fn only_content_events_trigger_a_reload() {
        assert!(is_content_change(EventKind::Create(CreateKind::File)));
        assert!(is_content_change(EventKind::Modify(ModifyKind::Any)));
        assert!(is_content_change(EventKind::Remove(RemoveKind::File)));

        assert!(!is_content_change(EventKind::Access(AccessKind::Read)));
        assert!(!is_content_change(EventKind::Any));
    }

    #[test]
    fn the_debounce_is_long_enough_to_span_an_editor_save() {
        assert!(DEBOUNCE >= Duration::from_millis(100));
        assert!(
            DEBOUNCE <= Duration::from_millis(500),
            "a reload should still feel immediate"
        );
    }
}
