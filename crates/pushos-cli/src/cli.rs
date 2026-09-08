//! The command line.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// A programmable operating layer for the Ableton Push 2.
#[derive(Debug, Parser)]
#[command(name = "pushos", version, about, long_about = None)]
pub(crate) struct Cli {
    /// Where configuration lives. Defaults to the PushOS configuration
    /// directory.
    #[arg(long, short, global = true, value_name = "PATH")]
    pub(crate) config: Option<PathBuf>,

    /// How much to log.
    #[arg(long, global = true, value_enum, default_value_t = Verbosity::Info)]
    pub(crate) log: Verbosity,

    /// What to do.
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// The available commands.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Run PushOS. Waits for the Push 2 and keeps running when it goes away.
    Run {
        /// Run against a simulated surface instead of the hardware.
        ///
        /// Useful for working on pages and bindings away from the device.
        #[arg(long)]
        fake: bool,
    },

    /// Check the configuration without running anything.
    Check,

    /// Report on the hardware, the configuration and the host integrations.
    Doctor,

    /// Write a starting configuration.
    Init {
        /// Overwrite an existing configuration.
        #[arg(long)]
        force: bool,
    },
}

/// How much to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum Verbosity {
    /// Only problems.
    Warn,
    /// What PushOS is doing.
    Info,
    /// Every decision, including bindings that matched nothing.
    Debug,
    /// Everything, including the input path.
    Trace,
}

impl Verbosity {
    /// The tracing filter this level corresponds to.
    ///
    /// Scoped to the PushOS crates so raising the level does not drown the
    /// operator in output from dependencies.
    pub(crate) const fn filter(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Info => "warn,pushos=info",
            Self::Debug => "warn,pushos=debug",
            Self::Trace => "warn,pushos=trace",
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn running_is_the_common_case_and_needs_no_arguments() {
        let cli = Cli::try_parse_from(["pushos", "run"]).expect("`pushos run` should parse");
        assert!(matches!(cli.command, Command::Run { fake: false }));
        assert_eq!(cli.log, Verbosity::Info);
    }

    #[test]
    fn a_configuration_path_may_be_given_before_or_after_the_command() {
        for arguments in [
            ["pushos", "--config", "/tmp/pushos", "check"],
            ["pushos", "check", "--config", "/tmp/pushos"],
        ] {
            let cli = Cli::try_parse_from(arguments).expect("both orders should parse");
            assert_eq!(
                cli.config.as_deref(),
                Some(std::path::Path::new("/tmp/pushos"))
            );
        }
    }

    #[test]
    fn every_verbosity_keeps_dependency_noise_down() {
        for level in [
            Verbosity::Warn,
            Verbosity::Info,
            Verbosity::Debug,
            Verbosity::Trace,
        ] {
            assert!(
                level.filter().starts_with("warn"),
                "`{level:?}` should leave dependencies at warn"
            );
        }
    }

    #[test]
    fn an_unknown_command_is_refused() {
        assert!(Cli::try_parse_from(["pushos", "explode"]).is_err());
    }
}
