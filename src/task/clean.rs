//! Cleaning targets: measurement and removal.

use std::path::PathBuf;

use serde::Serialize;

use super::{Ctx, Status, docker};
use crate::format;
use crate::sys::{cmd, fsx};

/// An area of the disk `detox-mac` knows how to measure and clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum Target {
    /// User caches (`~/Library/Caches`).
    Cache,
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
    /// Docker: unused containers, images and build caches (never volumes).
    Docker,
    /// Xcode data: DerivedData, DeviceSupport, simulator caches.
    Xcode,
    /// iOS simulator devices (heavy, must be asked for explicitly).
    Simulators,
}

impl Target {
    /// Targets covered by `all` (simulators are deliberately left out).
    pub const DEFAULT: [Target; 8] = [
        Target::Cache,
        Target::Trash,
        Target::TrashAll,
        Target::Logs,
        Target::DsStore,
        Target::Homebrew,
        Target::Docker,
        Target::Xcode,
    ];

    /// Targets measured by a bare `detox-mac scan`.
    pub const QUICK: [Target; 4] = [Target::Cache, Target::Trash, Target::Logs, Target::Xcode];

    pub fn label(self) -> &'static str {
        match self {
            Target::Cache => "User caches",
            Target::Trash => "Trash",
            Target::TrashAll => "Trash (all volumes)",
            Target::Logs => "Logs",
            Target::DsStore => ".DS_Store files",
            Target::Homebrew => "Homebrew cache",
            Target::Docker => "Docker (images, builds)",
            Target::Xcode => "Xcode data",
            Target::Simulators => "iOS simulators",
        }
    }

    /// Short name used on the command line.
    pub fn slug(self) -> &'static str {
        match self {
            Target::Cache => "cache",
            Target::Trash => "trash",
            Target::TrashAll => "trash-all",
            Target::Logs => "logs",
            Target::DsStore => "ds-store",
            Target::Homebrew => "homebrew",
            Target::Docker => "docker",
            Target::Xcode => "xcode",
            Target::Simulators => "simulators",
        }
    }

    fn strategy(self) -> Strategy {
        match self {
            Target::Cache => Strategy::Dirs(vec![fsx::home_join("Library/Caches")]),
            Target::Trash => Strategy::Dirs(vec![fsx::home_join(".Trash")]),
            Target::TrashAll => Strategy::Dirs(all_trashes()),
            Target::Logs => Strategy::Dirs(vec![fsx::home_join("Library/Logs")]),
            Target::DsStore => Strategy::DsStore,
            Target::Homebrew => Strategy::Homebrew,
            Target::Docker => Strategy::Docker,
            Target::Xcode => Strategy::Dirs(vec![
                fsx::home_join("Library/Developer/Xcode/DerivedData"),
                fsx::home_join("Library/Developer/Xcode/iOS DeviceSupport"),
                fsx::home_join("Library/Developer/Xcode/watchOS DeviceSupport"),
                fsx::home_join("Library/Developer/Xcode/tvOS DeviceSupport"),
                fsx::home_join("Library/Developer/CoreSimulator/Caches"),
            ]),
            Target::Simulators => Strategy::Dirs(vec![fsx::home_join(
                "Library/Developer/CoreSimulator/Devices",
            )]),
        }
    }
}

enum Strategy {
    /// Empty the contents of these directories.
    Dirs(Vec<PathBuf>),
    /// Walk the home directory looking for `.DS_Store` files.
    DsStore,
    /// Delegate to `brew cleanup`.
    Homebrew,
    /// Delegate to the `docker … prune` commands.
    Docker,
}

/// User trash plus the `.Trashes` of every mounted volume.
fn all_trashes() -> Vec<PathBuf> {
    let mut dirs = vec![fsx::home_join(".Trash")];
    if let Ok(volumes) = std::fs::read_dir("/Volumes") {
        for volume in volumes.flatten() {
            let trash = volume.path().join(".Trashes");
            if trash.is_dir() {
                dirs.push(trash);
            }
        }
    }
    dirs
}

/// Space taken by a target.
#[derive(Debug, Clone, Serialize)]
pub struct Measure {
    pub target: Target,
    pub label: &'static str,
    /// Reclaimable bytes.
    pub bytes: u64,
    /// Number of items involved.
    pub items: usize,
    /// Why the target is unavailable on this machine.
    pub unavailable: Option<String>,
}

impl Measure {
    fn unavailable(target: Target, reason: impl Into<String>) -> Self {
        Self {
            target,
            label: target.label(),
            bytes: 0,
            items: 0,
            unavailable: Some(reason.into()),
        }
    }
}

/// Measures the reclaimable space of a target, changing nothing.
pub fn measure(target: Target) -> Measure {
    match target.strategy() {
        Strategy::Dirs(dirs) => {
            let existing: Vec<&PathBuf> = dirs.iter().filter(|d| d.is_dir()).collect();
            if existing.is_empty() {
                return Measure::unavailable(target, "directory not found");
            }

            let mut bytes = 0;
            let mut items = 0;
            for dir in existing {
                bytes += fsx::size_of(dir);
                items += std::fs::read_dir(dir)
                    .map(|e| e.flatten().count())
                    .unwrap_or(0);
            }
            Measure {
                target,
                label: target.label(),
                bytes,
                items,
                unavailable: None,
            }
        }
        Strategy::DsStore => {
            let (bytes, items) = ds_store_files()
                .iter()
                .fold((0, 0), |(b, n), (_, size)| (b + size, n + 1));
            Measure {
                target,
                label: target.label(),
                bytes,
                items,
                unavailable: None,
            }
        }
        Strategy::Docker => match docker::availability().and_then(|()| docker::reclaimable()) {
            Ok(reclaimable) => Measure {
                target,
                label: target.label(),
                bytes: reclaimable.bytes,
                items: reclaimable.details.len(),
                unavailable: None,
            },
            Err(reason) => Measure::unavailable(target, reason),
        },
        Strategy::Homebrew => match homebrew_cache() {
            Ok(cache) => Measure {
                target,
                label: target.label(),
                bytes: fsx::size_of(&cache),
                items: std::fs::read_dir(&cache)
                    .map(|e| e.flatten().count())
                    .unwrap_or(0),
                unavailable: None,
            },
            Err(reason) => Measure::unavailable(target, reason),
        },
    }
}

/// Result of cleaning a target.
#[derive(Debug, Clone, Serialize)]
pub struct Cleaned {
    pub target: Target,
    pub label: &'static str,
    pub status: Status,
    /// Bytes freed (or that would be, in dry-run mode).
    pub freed: u64,
    /// Items removed.
    pub removed: usize,
    pub messages: Vec<String>,
}

impl Cleaned {
    fn skipped(target: Target, reason: impl Into<String>) -> Self {
        Self {
            target,
            label: target.label(),
            status: Status::Skipped,
            freed: 0,
            removed: 0,
            messages: vec![reason.into()],
        }
    }
}

/// Cleans a target.
pub fn clean(target: Target, ctx: &Ctx) -> Cleaned {
    match target.strategy() {
        Strategy::Dirs(dirs) => clean_dirs(target, &dirs, ctx),
        Strategy::DsStore => clean_ds_store(target, ctx),
        Strategy::Homebrew => clean_homebrew(target, ctx),
        Strategy::Docker => clean_docker(target, ctx),
    }
}

fn clean_docker(target: Target, ctx: &Ctx) -> Cleaned {
    if let Err(reason) = docker::availability() {
        return Cleaned::skipped(target, reason);
    }

    let reclaimable = docker::reclaimable().unwrap_or_default();

    if ctx.dry_run {
        let mut messages = reclaimable.details.clone();
        messages.extend(docker::planned_commands());
        return Cleaned {
            target,
            label: target.label(),
            status: Status::Simulated,
            freed: reclaimable.bytes,
            removed: 0,
            messages,
        };
    }

    if reclaimable.bytes == 0 {
        return Cleaned::skipped(target, "nothing to reclaim");
    }

    match docker::prune() {
        Ok(pruned) => Cleaned {
            target,
            label: target.label(),
            status: Status::Ok,
            freed: pruned.freed,
            removed: 0,
            messages: pruned.messages,
        },
        Err(err) => Cleaned {
            target,
            label: target.label(),
            status: Status::Failed,
            freed: 0,
            removed: 0,
            messages: vec![err],
        },
    }
}

fn clean_dirs(target: Target, dirs: &[PathBuf], ctx: &Ctx) -> Cleaned {
    let existing: Vec<&PathBuf> = dirs.iter().filter(|d| d.is_dir()).collect();
    if existing.is_empty() {
        return Cleaned::skipped(target, "directory not found");
    }

    let mut removal = fsx::Removal::default();
    let mut messages = Vec::new();

    for dir in existing {
        let result = fsx::empty_dir(dir, ctx.dry_run);
        if result.removed > 0 {
            messages.push(format!(
                "{} — {} ({} item(s))",
                format::tilde(dir),
                format::size(result.freed),
                result.removed
            ));
        }
        removal.merge(result);
    }

    finish(target, removal, messages, ctx)
}

fn clean_ds_store(target: Target, ctx: &Ctx) -> Cleaned {
    let mut removal = fsx::Removal::default();

    for (path, size) in ds_store_files() {
        match fsx::remove(&path, ctx.dry_run) {
            Ok(()) => {
                removal.freed += size;
                removal.removed += 1;
            }
            Err(err) => removal
                .errors
                .push(format!("{} : {err}", format::tilde(&path))),
        }
    }

    finish(target, removal, Vec::new(), ctx)
}

fn clean_homebrew(target: Target, ctx: &Ctx) -> Cleaned {
    let cache = match homebrew_cache() {
        Ok(cache) => cache,
        Err(reason) => return Cleaned::skipped(target, reason),
    };

    let before = fsx::size_of(&cache);
    if before == 0 {
        return Cleaned::skipped(target, "cache already empty");
    }

    if ctx.dry_run {
        return Cleaned {
            target,
            label: target.label(),
            status: Status::Simulated,
            freed: before,
            removed: 0,
            messages: vec!["brew cleanup --prune=all -s".to_string()],
        };
    }

    match cmd::run("brew", &["cleanup", "--prune=all", "-s"]) {
        Ok(_) => {
            let after = fsx::size_of(&cache);
            Cleaned {
                target,
                label: target.label(),
                status: Status::Ok,
                freed: before.saturating_sub(after),
                removed: 0,
                messages: vec![format::tilde(&cache)],
            }
        }
        Err(err) => Cleaned {
            target,
            label: target.label(),
            status: Status::Failed,
            freed: 0,
            removed: 0,
            messages: vec![err],
        },
    }
}

fn finish(target: Target, removal: fsx::Removal, mut messages: Vec<String>, ctx: &Ctx) -> Cleaned {
    let failed = removal.removed == 0 && !removal.errors.is_empty();
    messages.extend(removal.errors.iter().take(5).cloned());
    if removal.errors.len() > 5 {
        messages.push(format!("… and {} more error(s)", removal.errors.len() - 5));
    }

    let status = if failed {
        Status::Failed
    } else if removal.removed == 0 {
        Status::Skipped
    } else {
        Status::done(ctx.dry_run)
    };

    if status == Status::Skipped && messages.is_empty() {
        messages.push("nothing to clean".to_string());
    }

    Cleaned {
        target,
        label: target.label(),
        status,
        freed: removal.freed,
        removed: removal.removed,
        messages,
    }
}

/// Path of the Homebrew cache, or why there is none.
fn homebrew_cache() -> Result<PathBuf, String> {
    if !cmd::exists("brew") {
        return Err("Homebrew is not installed".to_string());
    }
    let cache = cmd::run("brew", &["--cache"])?;
    let path = PathBuf::from(cache.stdout);
    if path.is_dir() {
        Ok(path)
    } else {
        Err("cache already empty".to_string())
    }
}

/// Lists the `.DS_Store` files of the home directory with their size.
fn ds_store_files() -> Vec<(PathBuf, u64)> {
    let mut files = Vec::new();
    fsx::walk_files(&fsx::home(), 12, &mut |path, size| {
        if path.file_name().is_some_and(|name| name == ".DS_Store") {
            files.push((path.to_path_buf(), size));
        }
    });
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_target_has_a_distinct_slug() {
        let mut slugs: Vec<&str> = Target::DEFAULT.iter().map(|t| t.slug()).collect();
        slugs.push(Target::Simulators.slug());
        let count = slugs.len();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), count);
    }

    #[test]
    fn simulators_are_not_part_of_all() {
        assert!(!Target::DEFAULT.contains(&Target::Simulators));
    }
}
