//! Asking macOS how it is doing for memory.
//!
//! macOS keeps a pressure level for exactly this: applications are meant to
//! give memory back when it rises rather than wait to be killed. It is one
//! number, read with `sysctl`, and PushOS reads it seldom — once between
//! decisions about how many sessions may run, not once a frame.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pushos_domain::ports::{MemoryPressure, Pressure, ProcessRunner, ProcessSpec};
use tracing::debug;

/// Where macOS keeps the level.
const LEVEL: &str = "kern.memorystatus_vm_pressure_level";

/// Where `sysctl` is, by full path, so nothing on the path can stand in.
const SYSCTL: &str = "/usr/sbin/sysctl";

/// How long asking may take. It is a single read of a kernel value.
const PATIENCE: Duration = Duration::from_secs(5);

/// What macOS says about its memory.
#[derive(Debug)]
pub struct MacMemoryPressure {
    processes: Arc<dyn ProcessRunner>,
}

impl MacMemoryPressure {
    /// Asks this Mac.
    pub const fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self { processes }
    }
}

#[async_trait]
impl MemoryPressure for MacMemoryPressure {
    async fn now(&self) -> Pressure {
        let spec = ProcessSpec::new(SYSCTL, ["-n".to_owned(), LEVEL.to_owned()]).within(PATIENCE);
        let Ok(outcome) = self.processes.run(&spec).await else {
            return Pressure::Normal;
        };
        if !outcome.succeeded() {
            return Pressure::Normal;
        }
        let pressure = read(outcome.stdout_tail.trim());
        if pressure != Pressure::Normal {
            debug!(?pressure, "the Mac says memory is short");
        }
        pressure
    }
}

/// What the number macOS prints means.
///
/// The levels are a bit field: 1 is normal, 2 is a warning, 4 is critical.
/// Anything else is a level invented later, and is treated as room until it is
/// understood, because putting sessions away on a guess is worse than not.
fn read(printed: &str) -> Pressure {
    match printed.parse::<u32>() {
        Ok(4) => Pressure::Critical,
        Ok(2) => Pressure::Warning,
        _ => Pressure::Normal,
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::ports::ProcessOutcome;
    use pushos_testkit::FakeProcesses;

    use super::*;

    fn saying(printed: &str) -> Arc<FakeProcesses> {
        let processes = Arc::new(FakeProcesses::new());
        let said = printed.to_owned();
        processes.reply_with(move |_| ProcessOutcome {
            exit_code: Some(0),
            stdout_tail: said.clone(),
            stderr_tail: String::new(),
        });
        processes
    }

    #[tokio::test]
    async fn the_level_macos_prints_becomes_what_pushos_does_about_it() {
        for (printed, expected) in [
            ("1\n", Pressure::Normal),
            ("2\n", Pressure::Warning),
            ("4\n", Pressure::Critical),
        ] {
            let mac = MacMemoryPressure::new(saying(printed) as Arc<dyn ProcessRunner>);
            assert_eq!(mac.now().await, expected, "for {printed:?}");
        }
    }

    #[tokio::test]
    async fn a_mac_that_will_not_say_is_treated_as_having_room() {
        let processes = FakeProcesses::new();
        processes.set_exit_code(1);
        let mac = MacMemoryPressure::new(Arc::new(processes));
        assert_eq!(mac.now().await, Pressure::Normal);

        let nonsense = MacMemoryPressure::new(saying("later-levels") as Arc<dyn ProcessRunner>);
        assert_eq!(nonsense.now().await, Pressure::Normal);
    }

    #[tokio::test]
    async fn it_is_read_from_the_kernel_rather_than_worked_out() {
        let processes = saying("1");
        MacMemoryPressure::new(Arc::clone(&processes) as Arc<dyn ProcessRunner>)
            .now()
            .await;

        let spec = &processes.spawned()[0];
        assert_eq!(spec.program, SYSCTL);
        assert_eq!(spec.args, ["-n", LEVEL]);
    }
}
