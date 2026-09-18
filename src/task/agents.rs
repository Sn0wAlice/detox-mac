//! Agents et démons de démarrage (`launchd`).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{Ctx, Outcome, Status};
use crate::format;
use crate::sys::{cmd, fsx};

/// Emplacement d'un fichier `.plist`.
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
            Scope::User => "LaunchAgents utilisateur",
            Scope::System => "LaunchAgents système",
            Scope::Daemon => "LaunchDaemons système",
        }
    }

    pub fn dir(self) -> PathBuf {
        match self {
            Scope::User => fsx::home_join("Library/LaunchAgents"),
            Scope::System => PathBuf::from("/Library/LaunchAgents"),
            Scope::Daemon => PathBuf::from("/Library/LaunchDaemons"),
        }
    }

    /// Les démons système et les agents de `/Library` demandent root.
    pub fn needs_root(self) -> bool {
        !matches!(self, Scope::User)
    }
}

/// Un agent de démarrage.
#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    pub label: String,
    pub path: PathBuf,
    pub scope: Scope,
    /// `true` si le paquet vient d'Apple (jamais modifié par cet outil).
    pub apple: bool,
    /// `true` si `launchctl` le connaît comme chargé.
    pub loaded: bool,
}

/// Liste tous les agents visibles, triés par emplacement puis par label.
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

/// Labels actuellement chargés d'après `launchctl list`.
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

/// Un agent fourni par Apple ne doit jamais être désactivé ni supprimé.
fn is_apple(label: &str) -> bool {
    label.starts_with("com.apple.")
}

/// Action applicable à un agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Disable,
    Enable,
    Remove,
}

impl Action {
    fn verb(self) -> &'static str {
        match self {
            Action::Disable => "Désactivation",
            Action::Enable => "Réactivation",
            Action::Remove => "Suppression",
        }
    }
}

/// Applique une action à un agent.
pub fn apply(agent: &Agent, action: Action, ctx: &Ctx) -> Outcome {
    let name = format!("{} : {}", action.verb(), agent.label);

    if agent.apple {
        return Outcome::skipped(name, "agent Apple — ignoré par sécurité");
    }
    if agent.scope.needs_root() && !cmd::is_root() {
        return Outcome::skipped(name, "nécessite sudo");
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
        // `launchctl` râle souvent sur un service déjà (dé)chargé : ce n'est pas fatal.
        outcome.push(format!("launchctl : {err}"));
    }

    if action == Action::Remove {
        if let Err(err) = fsx::remove(&agent.path, false) {
            return Outcome::failed(name, format!("{} : {err}", format::tilde(&agent.path)));
        }
        outcome.push(format!("supprimé : {}", format::tilde(&agent.path)));
    }

    outcome
}

/// Compte les agents par origine.
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
