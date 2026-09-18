//! Command-line definition.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::format;
use crate::task::agents::Scope;
use crate::task::clean::Target;
use crate::task::schedule::Cadence;
use crate::ui::ColorChoice;

/// macOS maintenance tool: cleanup, diagnostics and startup agents.
#[derive(Debug, Parser)]
#[command(
    name = "detox",
    version,
    about,
    long_about = None,
    propagate_version = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(flatten)]
    pub options: Options,

    #[command(subcommand)]
    pub command: Command,
}

/// Options valid for every subcommand.
#[derive(Debug, Args)]
pub struct Options {
    /// Change nothing: measure and show what would be done.
    #[arg(short = 'n', long, global = true)]
    pub dry_run: bool,

    /// Answer yes to every confirmation.
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// Delete for good instead of moving to the trash.
    #[arg(long, global = true)]
    pub purge: bool,

    /// Never touch paths matching this glob (repeatable).
    #[arg(short = 'x', long, global = true, value_name = "GLOB")]
    pub exclude: Vec<String>,

    /// Ignore the configuration file.
    #[arg(long, global = true)]
    pub no_config: bool,

    /// Print warnings and errors only.
    #[arg(short, long, global = true, conflicts_with = "json")]
    pub quiet: bool,

    /// JSON output, for scripts.
    #[arg(long, global = true)]
    pub json: bool,

    /// When to colourise the output.
    #[arg(long, global = true, value_name = "WHEN", default_value = "auto")]
    pub color: ColorChoice,
}

/// A cleaning target as accepted on the command line, or `all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum TargetArg {
    /// Every target except the iOS simulators, which must be asked for.
    All,
    /// User caches (`~/Library/Caches`).
    Cache,
    /// Caches of sandboxed applications (`~/Library/Containers`).
    ContainerCache,
    /// Global caches of the package managers (npm, cargo, gradle…).
    PkgCache,
    /// User trash (`~/.Trash`).
    Trash,
    /// User trash plus the trash of every mounted volume.
    TrashAll,
    /// User logs (`~/Library/Logs`).
    Logs,
    /// `.DS_Store` files in the home directory.
    DsStore,
    /// Homebrew download cache.
    Homebrew,
    /// Docker: unused containers, images and build caches.
    Docker,
    /// Xcode data: DerivedData, DeviceSupport, simulator caches.
    Xcode,
    /// iOS simulator devices.
    Simulators,
    /// Local backups of iPhones and iPads.
    IosBackups,
    /// Virtual machines and container runtimes.
    Vm,
}

impl TargetArg {
    /// The concrete target, or `None` for `all`.
    fn target(self) -> Option<Target> {
        match self {
            TargetArg::All => None,
            TargetArg::Cache => Some(Target::Cache),
            TargetArg::ContainerCache => Some(Target::ContainerCache),
            TargetArg::PkgCache => Some(Target::PkgCache),
            TargetArg::Trash => Some(Target::Trash),
            TargetArg::TrashAll => Some(Target::TrashAll),
            TargetArg::Logs => Some(Target::Logs),
            TargetArg::DsStore => Some(Target::DsStore),
            TargetArg::Homebrew => Some(Target::Homebrew),
            TargetArg::Docker => Some(Target::Docker),
            TargetArg::Xcode => Some(Target::Xcode),
            TargetArg::Simulators => Some(Target::Simulators),
            TargetArg::IosBackups => Some(Target::IosBackups),
            TargetArg::Vm => Some(Target::Vm),
        }
    }

    /// Expands the arguments into concrete targets, in order and without duplicates.
    pub fn expand(args: &[TargetArg]) -> Vec<Target> {
        fn push(targets: &mut Vec<Target>, target: Target) {
            if !targets.contains(&target) {
                targets.push(target);
            }
        }

        let mut targets = Vec::new();
        for arg in args {
            match arg.target() {
                Some(target) => push(&mut targets, target),
                None => Target::DEFAULT
                    .iter()
                    .for_each(|&target| push(&mut targets, target)),
            }
        }

        targets
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Machine summary and reclaimable space.
    Info,

    /// Measure reclaimable space without deleting anything.
    Scan {
        /// Targets to measure; `all` measures everything (default: the quick ones).
        #[arg(value_name = "TARGET")]
        targets: Vec<TargetArg>,

        /// Ignore anything touched in the last N days.
        #[arg(short = 'o', long, value_name = "DAYS")]
        older_than: Option<u64>,
    },

    /// Clean one or more targets; `all` cleans everything at once.
    Clean {
        /// Targets to clean, or `all`.
        #[arg(value_name = "TARGET", required = true)]
        targets: Vec<TargetArg>,

        /// Leave alone anything touched in the last N days.
        #[arg(short = 'o', long, value_name = "DAYS")]
        older_than: Option<u64>,
    },

    /// List installed applications by size.
    Apps {
        /// Number of applications shown (0 = all).
        #[arg(short, long, value_name = "N", default_value_t = 15)]
        top: usize,

        /// Only those not opened for N days, according to Spotlight.
        #[arg(short, long, value_name = "DAYS")]
        unused: Option<u64>,
    },

    /// Explore files: large files and build residue.
    Files {
        #[command(subcommand)]
        command: FileCommand,
    },

    /// Inspect memory, grouped by application.
    Ram {
        /// Number of groups shown (0 = all).
        #[arg(short, long, value_name = "N", default_value_t = 15)]
        top: usize,

        /// Include macOS and Apple processes.
        #[arg(short, long)]
        all: bool,

        /// List the processes of every group.
        #[arg(short, long)]
        detail: bool,

        /// Hide groups below this size.
        #[arg(short, long, value_name = "SIZE", default_value = "0", value_parser = format::parse_size)]
        min: u64,
    },

    /// Detail the processes of one application.
    Inspect {
        /// Application name, or the number shown by `detox ram`.
        #[arg(value_name = "TARGET", required = true, num_args = 1..)]
        target: Vec<String>,

        /// Hide command lines.
        #[arg(short, long)]
        short: bool,
    },

    /// Stop every process of one application.
    Kill {
        /// Application name, or the number shown by `detox ram`.
        #[arg(value_name = "TARGET", required = true, num_args = 1..)]
        target: Vec<String>,

        /// Send SIGKILL instead of SIGTERM (no chance to save).
        #[arg(short, long)]
        force: bool,

        /// Allow targeting a macOS component or an Apple application.
        #[arg(long)]
        system: bool,
    },

    /// Remove an application and everything it left behind.
    Uninstall {
        /// Application name, as it appears in the Applications folder.
        #[arg(value_name = "APP", required = true, num_args = 1..)]
        target: Vec<String>,
    },

    /// Run a cleanup on its own, through launchd.
    Schedule {
        /// `daily`, `weekly`, or `off` to remove the schedule.
        /// Leave it out to show what is scheduled.
        #[arg(value_name = "WHEN")]
        when: Option<Cadence>,

        /// Targets to clean (default: cache, logs).
        #[arg(short, long, value_name = "TARGET", num_args = 1..)]
        targets: Vec<TargetArg>,

        /// Hour of the day it runs.
        #[arg(long, value_name = "HOUR", default_value_t = 3)]
        at: u32,
    },

    /// Find what uninstalled applications left behind.
    Orphans {
        /// Remove the leftovers instead of only listing them.
        #[arg(long)]
        clean: bool,

        /// Restrict to these bundle identifiers.
        #[arg(value_name = "BUNDLE_ID")]
        only: Vec<String>,

        /// Number of applications shown (0 = all).
        #[arg(short, long, value_name = "N", default_value_t = 20)]
        top: usize,
    },

    /// What was removed, and when.
    History {
        /// Number of runs shown (0 = all).
        #[arg(short, long, value_name = "N", default_value_t = 20)]
        top: usize,
    },

    /// Put back what a run moved to the trash.
    Undo {
        /// Identifier shown by `detox history` (default: the last run).
        #[arg(value_name = "ID")]
        id: Option<String>,
    },

    /// Manage startup agents and daemons.
    Agents {
        #[command(subcommand)]
        command: AgentCommand,
    },

    /// One-off system operations.
    Sys {
        #[command(subcommand)]
        command: SysCommand,
    },

    /// Generate shell completions.
    Completions {
        /// Target shell.
        #[arg(value_name = "SHELL")]
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Subcommand)]
pub enum FileCommand {
    /// List the largest files.
    Large {
        /// Minimum size (e.g. 500M, 1.5G).
        #[arg(short, long, value_name = "SIZE", default_value = "500M", value_parser = format::parse_size)]
        min: u64,

        /// Number of files shown (0 = all).
        #[arg(short, long, value_name = "N", default_value_t = 20)]
        top: usize,

        /// Starting directory (default: the home directory).
        #[arg(short, long, value_name = "PATH")]
        path: Option<PathBuf>,

        /// Maximum walk depth.
        #[arg(short, long, value_name = "N", default_value_t = 8)]
        depth: usize,
    },

    /// List installers and archives you downloaded and never opened.
    Downloads {
        /// Not opened for N days.
        #[arg(short = 'o', long, value_name = "DAYS", default_value_t = 180)]
        older_than: u64,

        /// Directory to look in (default: `~/Downloads`).
        #[arg(short, long, value_name = "PATH")]
        path: Option<PathBuf>,

        /// Every file, not only installers and archives.
        #[arg(short, long)]
        all: bool,

        /// Remove them instead of only listing them.
        #[arg(long)]
        clean: bool,

        /// Number of files shown (0 = all).
        #[arg(short, long, value_name = "N", default_value_t = 20)]
        top: usize,
    },

    /// List the build residue of local projects (read only).
    Dev {
        #[command(flatten)]
        filter: DevFilter,

        /// Number of directories shown (0 = all).
        #[arg(short, long, value_name = "N", default_value_t = 20)]
        top: usize,
    },

    /// Delete the build residue of local projects.
    Clean {
        #[command(flatten)]
        filter: DevFilter,

        /// Use the language's own tool when it exists (`cargo clean`,
        /// `swift package clean`…) instead of removing the directory.
        #[arg(long)]
        native: bool,

        /// Remove everything found without offering to keep any of it.
        #[arg(long)]
        all: bool,
    },
}

/// Which build residue to consider.
#[derive(Debug, Args)]
pub struct DevFilter {
    /// Keep only what has not changed for N days.
    #[arg(short = 'o', long, value_name = "DAYS", default_value_t = 7)]
    pub older_than: u64,

    /// Starting directory (default: the home directory).
    #[arg(short, long, value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Maximum walk depth.
    #[arg(short, long, value_name = "N", default_value_t = 8)]
    pub depth: usize,

    /// Restrict to some languages (rust, node, python…).
    #[arg(short, long, value_name = "LANGUAGE", num_args = 1..)]
    pub lang: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// List installed agents.
    List {
        /// Show third-party agents only.
        #[arg(short, long)]
        third_party: bool,

        /// Restrict to one location.
        #[arg(short, long, value_name = "LOCATION")]
        scope: Option<Scope>,
    },

    /// Disable third-party agents (`launchctl unload`, without deleting).
    Disable {
        #[command(flatten)]
        selection: Selection,
    },

    /// Re-enable third-party agents (`launchctl load`).
    Enable {
        #[command(flatten)]
        selection: Selection,
    },

    /// Permanently remove third-party agents.
    Remove {
        #[command(flatten)]
        selection: Selection,
    },
}

/// Agent selection: explicit labels, or every third-party agent.
#[derive(Debug, Args)]
pub struct Selection {
    /// Labels to act on (e.g. com.docker.helper).
    #[arg(value_name = "LABEL", required_unless_present = "all_third_party")]
    pub labels: Vec<String>,

    /// Every third-party agent. Apple agents are never touched.
    #[arg(long, conflicts_with = "labels")]
    pub all_third_party: bool,

    /// Restrict to one location.
    #[arg(short, long, value_name = "LOCATION")]
    pub scope: Option<Scope>,
}

#[derive(Debug, Subcommand)]
pub enum SysCommand {
    /// Flush the DNS cache (sudo).
    Dns,
    /// Rebuild the Spotlight index (sudo).
    Spotlight,
    /// Free inactive memory (sudo).
    Memory,
    /// Purge local Time Machine snapshots.
    Snapshots,
    /// Delete simulator devices whose runtime is gone.
    Simulators,
    /// List available macOS updates.
    Updates,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn all_expands_to_every_default_target() {
        assert_eq!(
            TargetArg::expand(&[TargetArg::All]),
            Target::DEFAULT.to_vec()
        );
    }

    #[test]
    fn expansion_keeps_order_and_drops_duplicates() {
        let targets = TargetArg::expand(&[TargetArg::Logs, TargetArg::All, TargetArg::Simulators]);
        assert_eq!(targets[0], Target::Logs);
        assert_eq!(targets.last(), Some(&Target::Simulators));
        assert_eq!(targets.iter().filter(|t| **t == Target::Logs).count(), 1);
    }
}
