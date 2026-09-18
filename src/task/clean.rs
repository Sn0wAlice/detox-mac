//! Cleaning targets: measurement and removal.

use std::path::PathBuf;

use serde::Serialize;

use super::{Ctx, Status, docker};
use crate::format;
use crate::sys::{
    cmd,
    fsx::{self, Disposal, Move, Policy},
};

/// How much a target is worth worrying about.
///
/// This is what separates "your next build puts it back" from "you will be
/// downloading this again", and it drives both the ordering of a scan and the
/// number of times the tool asks before acting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Risk {
    /// Pure cache: regenerated on demand, losing it costs nothing.
    Cache,
    /// Rebuildable, but it costs a download or a build.
    Rebuildable,
    /// Your data. Gone is gone.
    Data,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::Cache => "cache",
            Risk::Rebuildable => "rebuildable",
            Risk::Data => "data",
        }
    }
}

/// An area of the disk `detox-mac` knows how to measure and clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum Target {
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
    /// Docker: unused containers, images and build caches (never volumes).
    Docker,
    /// Xcode data: DerivedData, DeviceSupport, archives, simulator caches.
    Xcode,
    /// iOS simulator devices (heavy, must be asked for explicitly).
    Simulators,
    /// Local backups of iPhones and iPads (your data, asked for explicitly).
    IosBackups,
}

impl Target {
    /// Targets covered by `all`.
    ///
    /// Anything holding data the user cannot get back for free is left out:
    /// simulators and iOS backups have to be named.
    pub const DEFAULT: [Target; 10] = [
        Target::Cache,
        Target::ContainerCache,
        Target::PkgCache,
        Target::Trash,
        Target::TrashAll,
        Target::Logs,
        Target::DsStore,
        Target::Homebrew,
        Target::Docker,
        Target::Xcode,
    ];

    /// Targets measured by a bare `detox scan`.
    pub const QUICK: [Target; 5] = [
        Target::Cache,
        Target::PkgCache,
        Target::Trash,
        Target::Logs,
        Target::Xcode,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Target::Cache => "User caches",
            Target::ContainerCache => "Sandboxed app caches",
            Target::PkgCache => "Package manager caches",
            Target::Trash => "Trash",
            Target::TrashAll => "Trash (all volumes)",
            Target::Logs => "Logs",
            Target::DsStore => ".DS_Store files",
            Target::Homebrew => "Homebrew cache",
            Target::Docker => "Docker (images, builds)",
            Target::Xcode => "Xcode data",
            Target::Simulators => "iOS simulators",
            Target::IosBackups => "iOS device backups",
        }
    }

    /// Short name used on the command line.
    pub fn slug(self) -> &'static str {
        match self {
            Target::Cache => "cache",
            Target::ContainerCache => "container-cache",
            Target::PkgCache => "pkg-cache",
            Target::Trash => "trash",
            Target::TrashAll => "trash-all",
            Target::Logs => "logs",
            Target::DsStore => "ds-store",
            Target::Homebrew => "homebrew",
            Target::Docker => "docker",
            Target::Xcode => "xcode",
            Target::Simulators => "simulators",
            Target::IosBackups => "ios-backups",
        }
    }

    /// The target a command-line slug names.
    pub fn from_slug(slug: &str) -> Option<Target> {
        let mut all = Target::DEFAULT.to_vec();
        all.push(Target::Simulators);
        all.push(Target::IosBackups);
        all.into_iter().find(|target| target.slug() == slug)
    }

    /// Whether cleaning this target this way cannot be undone.
    pub fn is_irreversible(self, disposal: Disposal) -> bool {
        self.always_purges() || disposal == Disposal::Purge
    }

    /// What losing this target actually costs.
    pub fn risk(self) -> Risk {
        match self {
            Target::Cache
            | Target::ContainerCache
            | Target::Logs
            | Target::DsStore
            | Target::Homebrew => Risk::Cache,
            Target::PkgCache | Target::Docker | Target::Xcode | Target::Simulators => {
                Risk::Rebuildable
            }
            Target::Trash | Target::TrashAll | Target::IosBackups => Risk::Data,
        }
    }

    /// Whether the target needs Full Disk Access to be seen at all.
    pub fn needs_full_disk_access(self) -> bool {
        matches!(self, Target::ContainerCache | Target::IosBackups)
    }

    /// Targets for which moving to the trash makes no sense: emptying the
    /// trash into the trash, or piling up thousands of identically named
    /// `.DS_Store` files.
    fn always_purges(self) -> bool {
        matches!(self, Target::Trash | Target::TrashAll | Target::DsStore)
    }

    fn strategy(self) -> Strategy {
        match self {
            Target::Cache => Strategy::Dirs(vec![fsx::home_join("Library/Caches")]),
            Target::ContainerCache => Strategy::Dirs(container_caches()),
            Target::PkgCache => Strategy::Dirs(package_caches()),
            Target::Trash => Strategy::Dirs(vec![fsx::home_join(".Trash")]),
            Target::TrashAll => Strategy::Dirs(all_trashes()),
            Target::Logs => Strategy::Dirs(vec![fsx::home_join("Library/Logs")]),
            Target::DsStore => Strategy::DsStore,
            Target::Homebrew => Strategy::Homebrew,
            Target::Docker => Strategy::Docker,
            Target::Xcode => Strategy::Dirs(vec![
                fsx::home_join("Library/Developer/Xcode/DerivedData"),
                fsx::home_join("Library/Developer/Xcode/Archives"),
                fsx::home_join("Library/Developer/Xcode/iOS DeviceSupport"),
                fsx::home_join("Library/Developer/Xcode/watchOS DeviceSupport"),
                fsx::home_join("Library/Developer/Xcode/tvOS DeviceSupport"),
                fsx::home_join("Library/Developer/CoreSimulator/Caches"),
            ]),
            Target::Simulators => Strategy::Dirs(vec![fsx::home_join(
                "Library/Developer/CoreSimulator/Devices",
            )]),
            Target::IosBackups => Strategy::Dirs(vec![fsx::home_join(
                "Library/Application Support/MobileSync/Backup",
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

/// Global caches of the package managers.
///
/// All of it is re-downloadable, and all of it is invisible to `files dev`,
/// which only ever looks at what sits inside a project.
fn package_caches() -> Vec<PathBuf> {
    [
        // JavaScript
        ".npm/_cacache",
        "Library/Caches/Yarn",
        ".cache/yarn",
        "Library/pnpm/store",
        ".local/share/pnpm/store",
        ".bun/install/cache",
        // Rust
        ".cargo/registry/cache",
        ".cargo/registry/src",
        // Go
        "go/pkg/mod/cache/download",
        // JVM
        ".gradle/caches",
        ".m2/repository",
        // Python
        "Library/Caches/pip",
        ".cache/pip",
        "Library/Caches/uv",
        // Swift and Objective-C
        "Library/Caches/org.swift.swiftpm",
        "Library/Caches/CocoaPods",
        // PHP, .NET, Dart
        "Library/Caches/composer",
        ".nuget/packages",
        ".pub-cache",
    ]
    .iter()
    .map(|suffix| fsx::home_join(suffix))
    .collect()
}

/// Caches that sandboxed applications keep inside their own container, out of
/// reach of the plain `~/Library/Caches` sweep.
fn container_caches() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for root in ["Library/Containers", "Library/Group Containers"] {
        let Ok(entries) = std::fs::read_dir(fsx::home_join(root)) else {
            continue;
        };
        for entry in entries.flatten() {
            let cache = entry.path().join("Data/Library/Caches");
            if cache.is_dir() {
                dirs.push(cache);
            }
            let group_cache = entry.path().join("Library/Caches");
            if group_cache.is_dir() {
                dirs.push(group_cache);
            }
        }
    }
    dirs
}

/// User trash plus the trash of every mounted volume — the current user's own,
/// never the other accounts' on that volume.
fn all_trashes() -> Vec<PathBuf> {
    let mut dirs = vec![fsx::home_join(".Trash")];
    let uid = fsx::uid();

    if let Ok(volumes) = std::fs::read_dir("/Volumes") {
        for volume in volumes.flatten() {
            let mine = volume.path().join(".Trashes").join(uid.to_string());
            if mine.is_dir() {
                dirs.push(mine);
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
    pub risk: Risk,
    /// Reclaimable bytes, counted as blocks on disk.
    pub bytes: u64,
    /// Number of items involved.
    pub items: usize,
    /// Directories that could not be read.
    pub denied: usize,
    /// Why the target is unavailable on this machine.
    pub unavailable: Option<String>,
}

impl Measure {
    fn new(target: Target, bytes: u64, items: usize, denied: usize) -> Self {
        Self {
            target,
            label: target.label(),
            risk: target.risk(),
            bytes,
            items,
            denied,
            unavailable: None,
        }
    }

    fn unavailable(target: Target, reason: impl Into<String>) -> Self {
        Self {
            target,
            label: target.label(),
            risk: target.risk(),
            bytes: 0,
            items: 0,
            denied: 0,
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

            // One sizer for every directory of the target: a file hard-linked
            // between two package stores is counted once.
            let mut sizer = fsx::Sizer::new();
            let mut bytes = 0;
            let mut items = 0;

            for dir in existing {
                bytes += sizer.size_of(dir);
                items += std::fs::read_dir(dir)
                    .map(|entries| entries.flatten().count())
                    .unwrap_or(0);
            }

            let mut measure = Measure::new(target, bytes, items, sizer.denied);
            if sizer.denied > 0 && bytes == 0 {
                measure.unavailable =
                    Some(format!("unreadable — {}", fsx::full_disk_access_hint()));
            }
            measure
        }
        Strategy::DsStore => {
            let (files, denied) = ds_store_files();
            let bytes = files.iter().map(|(_, size)| size).sum();
            Measure::new(target, bytes, files.len(), denied)
        }
        Strategy::Docker => match docker::availability().and_then(|()| docker::reclaimable()) {
            Ok(reclaimable) => {
                Measure::new(target, reclaimable.bytes, reclaimable.details.len(), 0)
            }
            Err(reason) => Measure::unavailable(target, reason),
        },
        Strategy::Homebrew => match homebrew_cache() {
            Ok(cache) => Measure::new(
                target,
                fsx::size_of(&cache),
                std::fs::read_dir(&cache)
                    .map(|entries| entries.flatten().count())
                    .unwrap_or(0),
                0,
            ),
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
    /// What was done to the entries.
    pub disposal: Disposal,
    /// Bytes removed for good (or that would be, in dry-run mode).
    pub freed: u64,
    /// Bytes moved to the trash: recoverable, reclaimed once it is emptied.
    pub trashed: u64,
    /// Items removed.
    pub removed: usize,
    /// Items the configuration protected.
    pub excluded: usize,
    pub messages: Vec<String>,
    /// Trash moves, kept for the journal rather than for display.
    #[serde(skip)]
    pub moves: Vec<Move>,
}

impl Cleaned {
    /// Space the run accounted for, wherever it went.
    pub fn total(&self) -> u64 {
        self.freed + self.trashed
    }

    fn skipped(target: Target, reason: impl Into<String>) -> Self {
        Self {
            target,
            label: target.label(),
            status: Status::Skipped,
            disposal: Disposal::Purge,
            freed: 0,
            trashed: 0,
            removed: 0,
            excluded: 0,
            messages: vec![reason.into()],
            moves: Vec::new(),
        }
    }
}

/// Cleans a target.
pub fn clean(target: Target, ctx: &Ctx) -> Cleaned {
    // Some targets can only ever be purged, whatever the user asked for.
    let policy = if target.always_purges() {
        ctx.policy().purging()
    } else {
        ctx.policy()
    };

    match target.strategy() {
        Strategy::Dirs(dirs) => clean_dirs(target, &dirs, &policy, ctx),
        Strategy::DsStore => clean_ds_store(target, &policy, ctx),
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
            status: Status::Simulated,
            freed: reclaimable.bytes,
            messages,
            ..Cleaned::skipped(target, "")
        };
    }

    if reclaimable.bytes == 0 {
        return Cleaned::skipped(target, "nothing to reclaim");
    }

    match docker::prune() {
        Ok(pruned) => Cleaned {
            status: Status::Ok,
            freed: pruned.freed,
            messages: pruned.messages,
            ..Cleaned::skipped(target, "")
        },
        Err(err) => Cleaned {
            status: Status::Failed,
            messages: vec![err],
            ..Cleaned::skipped(target, "")
        },
    }
}

fn clean_dirs(target: Target, dirs: &[PathBuf], policy: &Policy, ctx: &Ctx) -> Cleaned {
    let existing: Vec<&PathBuf> = dirs.iter().filter(|d| d.is_dir()).collect();
    if existing.is_empty() {
        return Cleaned::skipped(target, "directory not found");
    }

    let mut removal = fsx::Removal::default();
    let mut messages = Vec::new();

    for dir in existing {
        let result = fsx::empty_dir(dir, policy);
        if result.removed > 0 {
            messages.push(format!(
                "{} — {} ({} item(s))",
                format::tilde(dir),
                format::size(result.total()),
                result.removed
            ));
        }
        removal.merge(result);
    }

    finish(target, removal, messages, policy, ctx)
}

fn clean_ds_store(target: Target, policy: &Policy, ctx: &Ctx) -> Cleaned {
    let mut removal = fsx::Removal::default();
    let (files, _) = ds_store_files();

    for (path, _) in files {
        match fsx::remove(&path, policy) {
            Ok(result) => removal.merge(result),
            Err(err) => removal.errors.push(err),
        }
    }

    finish(target, removal, Vec::new(), policy, ctx)
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
            status: Status::Simulated,
            freed: before,
            messages: vec!["brew cleanup --prune=all -s".to_string()],
            ..Cleaned::skipped(target, "")
        };
    }

    match cmd::run("brew", &["cleanup", "--prune=all", "-s"]) {
        Ok(_) => {
            let after = fsx::size_of(&cache);
            Cleaned {
                status: Status::Ok,
                freed: before.saturating_sub(after),
                messages: vec![format::tilde(&cache)],
                ..Cleaned::skipped(target, "")
            }
        }
        Err(err) => Cleaned {
            status: Status::Failed,
            messages: vec![err],
            ..Cleaned::skipped(target, "")
        },
    }
}

fn finish(
    target: Target,
    removal: fsx::Removal,
    mut messages: Vec<String>,
    policy: &Policy,
    ctx: &Ctx,
) -> Cleaned {
    let failed = removal.removed == 0 && !removal.errors.is_empty();

    if removal.excluded > 0 {
        messages.push(format!(
            "{} item(s) left alone by your exclusions",
            removal.excluded
        ));
    }
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
        disposal: policy.disposal,
        freed: removal.freed,
        trashed: removal.trashed,
        removed: removal.removed,
        excluded: removal.excluded,
        messages,
        moves: removal.moves,
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

/// The `.DS_Store` files of the home directory with their size, and how many
/// directories refused to be read along the way.
fn ds_store_files() -> (Vec<(PathBuf, u64)>, usize) {
    let mut files = Vec::new();
    let sizer = fsx::walk_files(&fsx::home(), 12, &mut |path, size| {
        if path.file_name().is_some_and(|name| name == ".DS_Store") {
            files.push((path.to_path_buf(), size));
        }
    });
    (files, sizer.denied)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every target that can be named on the command line.
    fn every_target() -> Vec<Target> {
        let mut targets = Target::DEFAULT.to_vec();
        targets.push(Target::Simulators);
        targets.push(Target::IosBackups);
        targets
    }

    #[test]
    fn every_target_has_a_distinct_slug() {
        let mut slugs: Vec<&str> = every_target().iter().map(|t| t.slug()).collect();
        let count = slugs.len();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), count);
    }

    #[test]
    fn all_leaves_out_what_cannot_be_downloaded_again() {
        assert!(!Target::DEFAULT.contains(&Target::Simulators));
        assert!(!Target::DEFAULT.contains(&Target::IosBackups));
    }

    #[test]
    fn nothing_holding_user_data_is_swept_by_all() {
        // The trash is the exception: emptying it is the point.
        for target in Target::DEFAULT {
            if matches!(target, Target::Trash | Target::TrashAll) {
                continue;
            }
            assert_ne!(target.risk(), Risk::Data, "{} holds data", target.slug());
        }
    }

    #[test]
    fn the_trash_is_never_moved_to_the_trash() {
        assert!(Target::Trash.always_purges());
        assert!(Target::TrashAll.always_purges());
        assert!(!Target::Cache.always_purges());
    }

    #[test]
    fn volume_trashes_belong_to_the_current_user() {
        // Never the whole `.Trashes`, which holds every account's deletions.
        for dir in all_trashes().iter().skip(1) {
            assert!(dir.ends_with(fsx::uid().to_string()));
        }
    }
}
