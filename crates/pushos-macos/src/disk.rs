//! Asking macOS how much room is left on a volume.
//!
//! Read with `df`, the same way the memory pressure level is read with
//! `sysctl`: one short subprocess, asked before PushOS takes more room rather
//! than on any loop.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pushos_domain::ports::{FreeSpace, ProcessRunner, ProcessSpec};
use tracing::debug;

/// Where `df` is, by full path, so nothing on the path can stand in.
const DF: &str = "/bin/df";

/// How long asking may take. It is one question about one volume.
const PATIENCE: Duration = Duration::from_secs(5);

/// How many bytes are in the blocks `df -k` counts.
const BLOCK: u64 = 1024;

/// What macOS says about the room on a volume.
#[derive(Debug)]
pub struct MacFreeSpace {
    processes: Arc<dyn ProcessRunner>,
}

impl MacFreeSpace {
    /// Asks this Mac.
    pub const fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self { processes }
    }
}

#[async_trait]
impl FreeSpace for MacFreeSpace {
    async fn at(&self, path: &Path) -> Option<u64> {
        let spec = ProcessSpec::new(DF, ["-k".to_owned(), path.to_string_lossy().into_owned()])
            .within(PATIENCE);

        let outcome = self.processes.run(&spec).await.ok()?;
        if !outcome.succeeded() {
            debug!(path = %path.display(), "the Mac would not say how much room is left");
            return None;
        }
        free_from(&outcome.stdout_tail)
    }
}

/// Reads the free blocks out of what `df -k` printed.
///
/// A heading, then one line for the volume: filesystem, blocks, used,
/// available, capacity, mount point. The fourth column is the answer, and
/// anything that does not look like that is no answer rather than a guess.
fn free_from(printed: &str) -> Option<u64> {
    let line = printed.lines().nth(1)?;
    let available: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    available.checked_mul(BLOCK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_available_column_is_the_one_read() {
        let printed = "Filesystem 1024-blocks      Used Available Capacity  Mounted on\n\
                       /dev/disk3s5  239362496 155083264  61234568    72%    /System/Volumes/Data\n";
        assert_eq!(free_from(printed), Some(61_234_568 * 1024));
    }

    #[test]
    fn anything_that_is_not_a_listing_is_no_answer_rather_than_a_guess() {
        // Nought free would stop PushOS working; "cannot say" lets it carry on.
        assert_eq!(free_from(""), None);
        assert_eq!(free_from("df: /nowhere: No such file or directory\n"), None);
        assert_eq!(
            free_from("Filesystem Blocks\n/dev/disk3s5 239362496\n"),
            None
        );
    }

    #[tokio::test]
    async fn against_this_mac() {
        let space = MacFreeSpace::new(Arc::new(crate::process::SystemProcessRunner));
        let free = space
            .at(Path::new("/tmp"))
            .await
            .expect("this Mac has a /tmp on a volume df can describe");

        assert!(free > 0, "a machine with nothing free could not be running");
    }
}
