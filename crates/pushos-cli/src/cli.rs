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

    /// Ask a running PushOS what it is doing.
    Status,

    /// List what a running PushOS has bound.
    Bindings,

    /// List the agent and terminal sessions a running PushOS is driving.
    Sessions,

    /// List the projects a running PushOS knows about.
    Workspaces,

    /// List the configurations PushOS ships with.
    Presets,

    /// Install and manage packs of agents, workflows, pages and bindings.
    Pack {
        /// What to do with them.
        #[command(subcommand)]
        command: PackCommand,
    },

    /// Write a starting configuration.
    Init {
        /// Which one to write. `pushos presets` lists them.
        #[arg(long, value_name = "NAME", default_value = crate::presets::DEFAULT)]
        preset: String,

        /// Overwrite an existing configuration.
        #[arg(long)]
        force: bool,
    },
}

/// What to do with packs.
#[derive(Debug, Subcommand)]
pub(crate) enum PackCommand {
    /// List the packs installed here.
    List,

    /// Say what a pack is, what it adds and what it asks for.
    ///
    /// Reads the pack and changes nothing.
    Show {
        /// The directory the pack is in.
        path: PathBuf,
    },

    /// Install a pack, after showing what it would change.
    Install {
        /// The directory the pack is in.
        path: PathBuf,

        /// Grant what it asks for without being asked to confirm.
        ///
        /// For scripts. Interactively, PushOS shows the review and waits.
        #[arg(long, short = 'y')]
        yes: bool,

        /// Replace a pack of the same name that is already installed.
        #[arg(long)]
        force: bool,
    },

    /// Remove an installed pack.
    Remove {
        /// Which one.
        pack: String,
    },

    /// Bring an installed pack back into force.
    Enable {
        /// Which one.
        pack: String,
    },

    /// Keep an installed pack but stop loading it.
    Disable {
        /// Which one.
        pack: String,
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
    fn every_read_only_command_needs_no_arguments() {
        for name in [
            "status",
            "bindings",
            "sessions",
            "workspaces",
            "check",
            "doctor",
            "presets",
        ] {
            Cli::try_parse_from(["pushos", name])
                .unwrap_or_else(|error| panic!("`pushos {name}` should parse: {error}"));
        }
    }

    #[test]
    fn writing_a_configuration_needs_no_arguments() {
        let cli = Cli::try_parse_from(["pushos", "init"]).expect("`pushos init` should parse");
        let Command::Init { preset, force } = cli.command else {
            panic!("`pushos init` should be the init command");
        };
        assert_eq!(preset, crate::presets::DEFAULT);
        assert!(!force);
    }

    #[test]
    fn a_preset_can_be_named() {
        let cli = Cli::try_parse_from(["pushos", "init", "--preset", "developer"])
            .expect("naming a preset should parse");
        let Command::Init { preset, .. } = cli.command else {
            panic!("init");
        };
        assert_eq!(preset, "developer");
    }

    #[test]
    fn every_pack_command_parses() {
        for arguments in [
            vec!["pushos", "pack", "list"],
            vec!["pushos", "pack", "show", "/tmp/seo"],
            vec!["pushos", "pack", "install", "/tmp/seo"],
            vec!["pushos", "pack", "install", "/tmp/seo", "--yes", "--force"],
            vec!["pushos", "pack", "remove", "seo"],
            vec!["pushos", "pack", "enable", "seo"],
            vec!["pushos", "pack", "disable", "seo"],
        ] {
            Cli::try_parse_from(&arguments)
                .unwrap_or_else(|error| panic!("`{arguments:?}` should parse: {error}"));
        }
    }

    #[test]
    fn installing_a_pack_asks_before_it_grants_anything() {
        // The default has to be the safe one: a script opts in with --yes, and
        // a person is shown what would change.
        let cli = Cli::try_parse_from(["pushos", "pack", "install", "/tmp/seo"]).expect("parses");
        let Command::Pack {
            command: PackCommand::Install { yes, force, .. },
        } = cli.command
        else {
            panic!("install");
        };
        assert!(!yes);
        assert!(!force);
    }

    #[test]
    fn an_unknown_command_is_refused() {
        assert!(Cli::try_parse_from(["pushos", "explode"]).is_err());
    }
}
