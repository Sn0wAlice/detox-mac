//! Startup agents and daemons (`launchd`).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{Ctx, Outcome, Status};
use crate::format;
use crate::sys::{cmd, fsx};

/// Where a `.plist` file lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum Scope {
    /// `~/Library/LaunchAgents`
    User,
    /// `/Library/LaunchAgents`
    System,
    /// `/Library/LaunchDaemons`
    Daemon,
}

impl Scope {
    pub const ALL: [Scope; 3] = [Scope::User, Scope::System, Scope::Daemon];

    pub fn label(self) -> &'static str {
        match self {
            Scope::User => "User LaunchAgents",
            Scope::System => "System LaunchAgents",
            Scope::Daemon => "System LaunchDaemons",
        }
    }

    pub fn dir(self) -> PathBuf {
        match self {
            Scope::User => fsx::home_join("Library/LaunchAgents"),
            Scope::System => PathBuf::from("/Library/LaunchAgents"),
            Scope::Daemon => PathBuf::from("/Library/LaunchDaemons"),
        }
    }

    /// System daemons and `/Library` agents need root.
    pub fn needs_root(self) -> bool {
        !matches!(self, Scope::User)
    }
}

/// A startup agent.
#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    pub label: String,
    pub path: PathBuf,
    pub scope: Scope,
    /// `true` when the package comes from Apple (never modified by this tool).
    pub apple: bool,
    /// `true` when `launchctl` reports it as loaded.
    pub loaded: bool,
}

/// Lists every visible agent, ordered by location then by label.
pub fn collect() -> Vec<Agent> {
    let loaded = loaded_labels();
    let mut agents = Vec::new();

    for scope in Scope::ALL {
        for path in fsx::plists(&scope.dir()) {
            let label = label_of(&path);
            agents.push(Agent {
                apple: is_apple(&label),
                loaded: loaded.contains(&label),
                label,
                path,
                scope,
            });
        }
    }

    agents
}

/// Labels currently loaded according to `launchctl list`.
fn loaded_labels() -> HashSet<String> {
    let Ok(output) = cmd::run("launchctl", &["list"]) else {
        return HashSet::new();
    };

    output
        .stdout
        .lines()
        .skip(1)
        .filter_map(|line| line.split('\t').nth(2))
        .map(str::to_string)
        .collect()
}

fn label_of(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// An Apple-provided agent must never be disabled nor removed.
fn is_apple(label: &str) -> bool {
    label.starts_with("com.apple.")
}

/// Action that can be applied to an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Disable,
    Enable,
    Remove,
}

impl Action {
    fn verb(self) -> &'static str {
        match self {
            Action::Disable => "Disable",
            Action::Enable => "Enable",
            Action::Remove => "Remove",
        }
    }
}

/// Applies an action to an agent.
pub fn apply(agent: &Agent, action: Action, ctx: &Ctx) -> Outcome {
    let name = format!("{} {}", action.verb(), agent.label);

    if agent.apple {
        return Outcome::skipped(name, "Apple agent — skipped for safety");
    }
    if agent.scope.needs_root() && !cmd::is_root() {
        return Outcome::skipped(name, "requires sudo");
    }

    if ctx.dry_run {
        return Outcome::new(name, Status::Simulated).with(format::tilde(&agent.path));
    }

    let subcommand = match action {
        Action::Enable => "load",
        Action::Disable | Action::Remove => "unload",
    };

    let mut outcome = Outcome::ok(&name, false);
    let args = [
        OsStr::new(subcommand),
        OsStr::new("-w"),
        agent.path.as_os_str(),
    ];
    if let Err(err) = cmd::run("launchctl", &args) {
        // `launchctl` often complains about an already (un)loaded service; not fatal.
        outcome.push(format!("launchctl : {err}"));
    }

    if action == Action::Remove {
        // A definition file is small and easy to want back: it goes to the
        // trash unless the user asked for a permanent deletion.
        let policy = ctx.policy();
        match fsx::remove(&agent.path, &policy) {
            Ok(removal) => {
                let verb = if policy.disposal.is_trash() {
                    "moved to the trash"
                } else {
                    "removed"
                };
                let _ = removal;
                outcome.push(format!("{verb}: {}", format::tilde(&agent.path)));
            }
            Err(err) => return Outcome::failed(name, err),
        }
    }

    outcome
}

/// Counts agents by origin.
pub fn summary(agents: &[Agent]) -> (usize, usize) {
    let apple = agents.iter().filter(|a| a.apple).count();
    (apple, agents.len() - apple)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_apple_labels() {
        assert!(is_apple("com.apple.Spotlight"));
        assert!(!is_apple("com.docker.helper"));
    }

    #[test]
    fn label_comes_from_filename() {
        let path = PathBuf::from("/Library/LaunchAgents/com.docker.helper.plist");
        assert_eq!(label_of(&path), "com.docker.helper");
    }

    #[test]
    fn only_user_scope_runs_without_root() {
        assert!(!Scope::User.needs_root());
        assert!(Scope::System.needs_root());
        assert!(Scope::Daemon.needs_root());
    }
}
