//! Coordinated shutdown.
//!
//! Every long-running task holds a token. Cancelling it stops them all, and the
//! runtime waits for each to finish rather than leaving detached work behind.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{info, warn};

/// How long a task has to finish once shutdown begins.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Owns the runtime's tasks and the signal that stops them.
#[derive(Debug, Clone)]
pub struct Shutdown {
    token: CancellationToken,
    tasks: TaskTracker,
}

impl Shutdown {
    /// Builds a fresh coordinator.
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }
    }

    /// The token every task should watch.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// A coordinator for work that ends before the runtime does.
    ///
    /// Cancelling the child stops only what it owns; cancelling the parent
    /// stops both. Used for the tasks that belong to one surface, which comes
    /// and goes while everything above it keeps running.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
            tasks: TaskTracker::new(),
        }
    }

    /// Registers a task so shutdown waits for it.
    ///
    /// Nothing important is spawned outside this, which is what stops a
    /// detached task from outliving the runtime that started it.
    pub fn spawn<F>(&self, task: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.tasks.spawn(task)
    }

    /// Begins shutdown without waiting for anything.
    ///
    /// This is what a signal handler calls. [`Self::stop`] waits for every
    /// registered task, which a task registered with the same coordinator
    /// cannot do without waiting for itself.
    pub fn begin(&self) {
        self.token.cancel();
    }

    /// Whether shutdown has begun.
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Waits until shutdown begins.
    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }

    /// Signals shutdown and waits for every registered task, up to the grace
    /// period.
    ///
    /// Returns whether everything stopped in time. A task that overran is
    /// reported rather than waited on forever.
    pub async fn stop(self) -> bool {
        info!("shutting down");
        self.token.cancel();
        self.tasks.close();

        if tokio::time::timeout(SHUTDOWN_GRACE, self.tasks.wait())
            .await
            .is_err()
        {
            warn!(
                remaining = self.tasks.len(),
                "some tasks did not stop within the grace period"
            );
            return false;
        }
        true
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn cancelling_stops_every_registered_task() {
        let shutdown = Shutdown::new();
        let stopped = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let token = shutdown.token();
            let stopped = Arc::clone(&stopped);
            shutdown.spawn(async move {
                token.cancelled().await;
                stopped.fetch_add(1, Ordering::SeqCst);
            });
        }

        assert!(
            shutdown.clone().stop().await,
            "well-behaved tasks stop in time"
        );
        assert_eq!(stopped.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_registered_task_can_begin_shutdown_without_waiting_for_itself() {
        let shutdown = Shutdown::new();

        let trigger = shutdown.clone();
        shutdown.spawn(async move {
            trigger.begin();
        });

        let watcher = shutdown.clone();
        let observed = shutdown.spawn(async move {
            watcher.cancelled().await;
            true
        });

        assert!(shutdown.clone().stop().await, "shutdown should complete");
        assert!(observed.await.expect("the task finished"));
    }

    #[tokio::test]
    async fn a_task_that_finishes_early_is_not_waited_on_twice() {
        let shutdown = Shutdown::new();
        shutdown.spawn(async { 42 });
        assert!(shutdown.stop().await);
    }

    #[tokio::test(start_paused = true)]
    async fn a_task_that_ignores_the_signal_is_reported_rather_than_waited_on_forever() {
        let shutdown = Shutdown::new();
        shutdown.spawn(async {
            // Deliberately ignores the token.
            tokio::time::sleep(Duration::from_secs(3_600)).await;
        });

        assert!(!shutdown.stop().await, "the overrun should be reported");
    }

    #[tokio::test]
    async fn cancellation_is_visible_to_a_task_that_checks_it() {
        let shutdown = Shutdown::new();
        assert!(!shutdown.is_cancelled());

        let watcher = shutdown.clone();
        let handle = shutdown.spawn(async move {
            watcher.cancelled().await;
            watcher.is_cancelled()
        });

        shutdown.clone().stop().await;
        assert!(handle.await.expect("the task finished"));
    }
}
