//! Asking the machine how much room is left.
//!
//! PushOS writes working trees to disk, and a working tree is as large as the
//! project in it. A surface that kept making them until the disk was full
//! would not merely stop working: a Mac with no room left cannot save, cannot
//! swap and cannot log, and every application on it starts failing at once.
//!
//! So this is asked before PushOS takes more room, and the answer is allowed
//! to be "cannot say". A machine that will not answer is treated as having
//! room, because refusing an operator their work on no evidence would be the
//! worse mistake.

use std::path::Path;

use async_trait::async_trait;

/// How much room is left where PushOS wants to write.
#[async_trait]
pub trait FreeSpace: Send + Sync + std::fmt::Debug {
    /// Bytes free on the volume holding `path`, when the host will say.
    ///
    /// `None` means the question could not be answered, which is not the same
    /// as nought and must never be treated as it.
    async fn at(&self, path: &Path) -> Option<u64>;
}

/// How little room may be left before PushOS stops taking more.
///
/// Below this a Mac is already in trouble — macOS needs room to swap, and
/// applications that cannot save start losing work. PushOS refusing to make
/// another working tree is a small, legible failure; being the thing that
/// took the last gigabyte is not.
pub const LEAST_FREE: u64 = 2 * 1024 * 1024 * 1024;

/// A machine that always says it has room, for tests and for anything that
/// does not know how to ask.
#[derive(Clone, Copy, Debug, Default)]
pub struct RoomOnDisk;

#[async_trait]
impl FreeSpace for RoomOnDisk {
    async fn at(&self, _path: &Path) -> Option<u64> {
        None
    }
}

/// Whether there is room to write where `path` is.
///
/// True when the host cannot say: see the note on [`FreeSpace::at`].
pub async fn room_at(space: &dyn FreeSpace, path: &Path) -> bool {
    space.at(path).await.is_none_or(|free| free >= LEAST_FREE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Holding(Option<u64>);

    #[async_trait]
    impl FreeSpace for Holding {
        async fn at(&self, _path: &Path) -> Option<u64> {
            self.0
        }
    }

    #[tokio::test]
    async fn a_machine_that_will_not_say_is_given_the_benefit_of_the_doubt() {
        // Refusing an operator their work because a `df` failed would be a
        // worse mistake than the one this guards against.
        assert!(room_at(&RoomOnDisk, Path::new("/tmp")).await);
        assert!(room_at(&Holding(None), Path::new("/tmp")).await);
    }

    #[tokio::test]
    async fn a_disk_with_nothing_left_is_not_written_to() {
        assert!(!room_at(&Holding(Some(0)), Path::new("/tmp")).await);
        assert!(!room_at(&Holding(Some(LEAST_FREE - 1)), Path::new("/tmp")).await);
        assert!(room_at(&Holding(Some(LEAST_FREE)), Path::new("/tmp")).await);
    }
}
