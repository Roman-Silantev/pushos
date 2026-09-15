//! What PushOS logs, where it goes, and the most room it may take.
//!
//! Run as an app, PushOS writes to standard error and launchd appends that to a
//! file. Nothing trims a file launchd appends to, so a PushOS left running for
//! months, or one stuck saying the same thing, would fill the disk a line at a
//! time. The file is therefore kept under a size here: once it passes the cap,
//! what it holds becomes the one older copy kept beside it, and it starts again
//! empty. Two files, each under the cap, is all the room logging ever takes.
//!
//! launchd opens the file for appending, so after it is emptied the next line
//! lands at its start rather than where the old end was.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Names the file standard error is appended to, when something is.
///
/// Set by the login agent PushOS installs. Nothing is trimmed without it,
/// because PushOS cannot tell a terminal from a file it may empty.
const LOG_FILE: &str = pushos_macos::app::LOG_FILE_VARIABLE;

/// The most one log file may hold before it is started again.
const CAP: u64 = 5 * 1024 * 1024;

/// How much is written between looks at the file's size.
///
/// A look is one `stat`. Every line would be wasteful, and this lets the file
/// pass the cap by at most this much.
const CHECK_EVERY: u64 = 64 * 1024;

/// Sets up logging, letting `RUST_LOG` override the chosen level.
///
/// `RUST_LOG` takes a level, or a level and per-module levels, such as
/// `info,pushos_push2=debug`. A value that does not read as one is ignored in
/// favour of the default rather than silencing everything.
pub(crate) fn install(default: &str) {
    use tracing_subscriber::filter::Targets;
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let keeper = kept_file();
    // Before anything is written, so a PushOS that fails at every start and is
    // started again every few seconds still cannot grow the file past the cap.
    if let Some(keeper) = &keeper {
        keeper.keep();
    }

    let filter = std::env::var("RUST_LOG")
        .ok()
        .and_then(|written| written.parse::<Targets>().ok())
        .or_else(|| default.parse::<Targets>().ok())
        .unwrap_or_default();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
                .with_writer(move || KeptWriter {
                    keeper: keeper.clone(),
                }),
        )
        .with(filter)
        .init();
}

/// The file standard error goes to, when the login agent said which.
fn kept_file() -> Option<Arc<Keeper>> {
    let path = std::env::var_os(LOG_FILE).filter(|value| !value.is_empty())?;
    Some(Arc::new(Keeper::new(PathBuf::from(path), CAP)))
}

/// Keeps one log file under a size.
#[derive(Debug)]
struct Keeper {
    path: PathBuf,
    cap: u64,
    since_look: AtomicU64,
    /// Held while starting the file again, so two threads never do it at once.
    starting_again: Mutex<()>,
}

impl Keeper {
    fn new(path: PathBuf, cap: u64) -> Self {
        Self {
            path,
            cap,
            since_look: AtomicU64::new(0),
            starting_again: Mutex::new(()),
        }
    }

    /// Notes that some bytes were written, looking at the file now and then.
    fn wrote(&self, bytes: usize) {
        let written = u64::try_from(bytes).unwrap_or(u64::MAX);
        let since = self.since_look.fetch_add(written, Ordering::Relaxed);
        if since.saturating_add(written) >= CHECK_EVERY {
            self.since_look.store(0, Ordering::Relaxed);
            self.keep();
        }
    }

    /// Starts the file again if it has passed the cap, keeping one older copy.
    fn keep(&self) {
        let Ok(_starting) = self.starting_again.try_lock() else {
            return;
        };
        let Ok(metadata) = std::fs::metadata(&self.path) else {
            return;
        };
        if metadata.len() <= self.cap {
            return;
        }
        // Copied rather than renamed: launchd holds the file open, and a rename
        // would leave it appending to the older copy instead.
        let _ = std::fs::copy(&self.path, older(&self.path));
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&self.path) {
            let _ = file.set_len(0);
        }
    }
}

/// Where the one older copy of a log is kept.
fn older(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

/// Standard error, telling the keeper what went through it.
struct KeptWriter {
    keeper: Option<Arc<Keeper>>,
}

impl std::io::Write for KeptWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = std::io::stderr().write(bytes)?;
        if let Some(keeper) = &self.keeper {
            keeper.wrote(written);
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("pushos-logging-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("writable");
        directory
    }

    #[test]
    fn a_log_past_its_cap_starts_again_and_keeps_one_older_copy() {
        let directory = scratch("cap");
        let path = directory.join("pushos.log");
        std::fs::write(&path, "x".repeat(2_000)).expect("writable");

        let keeper = Keeper::new(path.clone(), 1_000);
        keeper.keep();

        assert_eq!(std::fs::metadata(&path).expect("still there").len(), 0);
        assert_eq!(
            std::fs::read_to_string(older(&path)).expect("kept").len(),
            2_000
        );
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn a_log_under_its_cap_is_left_alone() {
        let directory = scratch("under");
        let path = directory.join("pushos.log");
        std::fs::write(&path, "short").expect("writable");

        Keeper::new(path.clone(), 1_000).keep();

        assert_eq!(std::fs::read_to_string(&path).expect("there"), "short");
        assert!(!older(&path).exists());
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn a_writer_appending_carries_on_at_the_start_once_the_file_starts_again() {
        // What launchd does with the file it gives PushOS as standard error.
        // Were the next line written where the old end was, the file would
        // keep its size with a hole in it, and nothing would have been freed.
        let directory = scratch("append");
        let path = directory.join("pushos.log");
        let mut appending = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("writable");
        appending.write_all(&[b'x'; 2_000]).expect("written");

        Keeper::new(path.clone(), 1_000).keep();
        appending.write_all(b"next line\n").expect("written");

        assert_eq!(
            std::fs::read_to_string(&path).expect("there"),
            "next line\n"
        );
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn the_file_is_looked_at_only_after_enough_has_been_written() {
        let directory = scratch("often");
        let path = directory.join("pushos.log");
        std::fs::write(&path, "x".repeat(2_000)).expect("writable");
        let keeper = Keeper::new(path.clone(), 1_000);

        keeper.wrote(10);
        assert_eq!(
            std::fs::metadata(&path).expect("there").len(),
            2_000,
            "one short line is not worth a look"
        );

        keeper.wrote(usize::try_from(CHECK_EVERY).expect("fits"));
        assert_eq!(std::fs::metadata(&path).expect("there").len(), 0);
        std::fs::remove_dir_all(directory).ok();
    }
}
