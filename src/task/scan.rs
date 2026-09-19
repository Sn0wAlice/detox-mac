//! Disk inventory: applications and large files.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;

use crate::format;
use crate::sys::{cmd, fsx};

/// An item weighed on disk.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// Display name (application) or shortened path (file).
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    /// Days since it was last used, when that could be established at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unused_days: Option<u64>,
    /// What `unused_days` was read from. Applications only: for a loose file
    /// Spotlight is the only source there is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
}

/// What a day count about an application actually rests on.
///
/// Worth carrying around because the three are not equally strong, and a tool
/// that proposes deletions has to say which one it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// Spotlight recorded the launch itself. The direct answer.
    Launch,
    /// No launch on record, but the application's own preferences, caches or
    /// saved state were written then, which takes a running application.
    Traces,
    /// Nothing on this machine remembers it running, so the count is the age
    /// of the bundle: how long it has sat there without leaving a trace.
    Install,
}

/// Installed application bundles, unmeasured.
///
/// Sizing is the slow part, so it is left to the caller: it can report
/// progress while walking this list.
pub fn application_bundles() -> Vec<PathBuf> {
    let roots = [
        PathBuf::from("/Applications"),
        fsx::home_join("Applications"),
    ];

    let mut apps = Vec::new();
    for root in roots.iter().filter(|root| root.is_dir()) {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "app") {
                apps.push(path);
            }
        }
    }

    apps.sort();
    apps
}

/// Measures one application bundle.
pub fn measure_application(path: &Path) -> Entry {
    Entry {
        name: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        bytes: fsx::size_of(path),
        path: path.to_path_buf(),
        unused_days: None,
        evidence: None,
    }
}

/// Files bigger than `min_size` under `root`, largest first.
///
/// `progress` is called regularly with the number of files seen so far.
pub fn large_files(
    root: &Path,
    min_size: u64,
    max_depth: usize,
    progress: &(impl Fn(usize, &Path) + Sync),
) -> Vec<Entry> {
    let files = Mutex::new(Vec::new());
    let seen = AtomicUsize::new(0);

    fsx::walk_files(root, max_depth, &|path, size| {
        let count = seen.fetch_add(1, Ordering::Relaxed) + 1;
        progress(count, path);

        if size >= min_size {
            if let Ok(mut found) = files.lock() {
                found.push(Entry {
                    name: format::tilde(path),
                    path: path.to_path_buf(),
                    bytes: size,
                    unused_days: None,
                    evidence: None,
                });
            }
        }
    });

    let mut files = files.into_inner().unwrap_or_default();
    files.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    files
}

/// Days since a file was last opened, according to Spotlight.
///
/// The modification time says when something was *written*; a downloaded
/// installer is written once and never touched again, whether it was run or
/// not. `kMDItemLastUsedDate` is the one that answers "do you use this?".
///
/// `None` when Spotlight has nothing on it, which is not the same as never
/// opened: the attribute is missing on an unindexed volume, and macOS also
/// lets it lapse on anything not opened recently. Silence here means unknown
/// — see [`application_last_use`] for what to do about it.
pub fn days_since_used(path: &Path) -> Option<u64> {
    let output = cmd::run(
        "mdls",
        &[
            "-name",
            "kMDItemLastUsedDate",
            "-raw",
            &path.to_string_lossy(),
        ],
    )
    .ok()?;

    let epoch = format::parse_spotlight_date(&output.stdout)?;
    Some(format::epoch_now().saturating_sub(epoch) / 86_400)
}

/// Days since an application was last used, and what that rests on.
///
/// Spotlight alone is not enough here. It keeps `kMDItemLastUsedDate` for
/// applications launched recently and drops it for the rest, so on a real
/// machine every application worth reporting comes back empty — and reading
/// that emptiness as "never opened" flags things the owner uses monthly.
///
/// So when Spotlight says nothing, ask the application's own files: running
/// one writes preferences, caches, saved state. Failing that, fall back to
/// the age of the bundle, which is a ceiling rather than an answer — an
/// application installed last week cannot have been idle for a year.
pub fn application_last_use(bundle: &Path) -> (u64, Evidence) {
    resolve(
        days_since_used(bundle),
        || days_since_traces(bundle),
        || fsx::age_days(bundle),
    )
}

/// Picks the strongest of the three signals that actually answered.
///
/// The later two are closures because reaching for them means walking the
/// home library, which is wasted work when Spotlight already knew.
fn resolve(
    launch: Option<u64>,
    traces: impl FnOnce() -> Option<u64>,
    install: impl FnOnce() -> u64,
) -> (u64, Evidence) {
    if let Some(days) = launch {
        return (days, Evidence::Launch);
    }
    match traces() {
        Some(days) => (days, Evidence::Traces),
        None => (install(), Evidence::Install),
    }
}

/// Days since anything belonging to an application was last written.
///
/// The most recent of its support files wins: caches go stale, preferences do
/// not, and either one being touched means the application ran.
fn days_since_traces(bundle: &Path) -> Option<u64> {
    let mut latest = 0;

    if let Some(bundle_id) = super::orphans::bundle_id(bundle) {
        latest = super::orphans::support_entries(&bundle_id, bundle)
            .iter()
            .map(|(_, path)| fsx::mtime(path))
            .max()
            .unwrap_or(0);
    }

    // Plenty of applications name their support directory after themselves
    // instead of after their identifier, and those tend to be the ones that
    // write to it constantly. A display name is too loose to delete by, so
    // `uninstall` keeps ignoring these — but it is safe to *read*: a wrong
    // match can only make an application look more recently used, while a
    // miss means offering to delete something in weekly use.
    let name = bundle.file_stem().unwrap_or_default();
    for dir in ["Application Support", "Caches", "Logs"] {
        latest = latest.max(fsx::mtime(
            &fsx::home_join(&format!("Library/{dir}")).join(name),
        ));
    }

    (latest > 0).then(|| format::epoch_now().saturating_sub(latest) / 86_400)
}

/// Extensions of the things people download, install once, and forget.
const INSTALLER_EXTENSIONS: &[&str] = &[
    "dmg", "pkg", "mpkg", "iso", "zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "msi", "exe",
    "deb", "rpm", "appimage",
];

/// Whether a name ends in one of those.
fn is_installer(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|ext| INSTALLER_EXTENSIONS.contains(&ext.as_str()))
}

/// Files in a directory that have not been opened for a while.
///
/// Only the top level: an installer is a single file, and descending into a
/// downloaded project directory would just be `files large` again.
pub fn dormant(
    root: &Path,
    min_days: u64,
    installers_only: bool,
    progress: &(impl Fn(usize, &Path) + Sync),
) -> Vec<Entry> {
    let Ok(entries) = fsx::entries_of(root) else {
        return Vec::new();
    };

    let candidates: Vec<PathBuf> = entries
        .into_iter()
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .filter(|path| !installers_only || path.is_dir() || is_installer(path))
        .collect();

    let seen = AtomicUsize::new(0);
    let found = crate::sys::par::map(
        &candidates,
        |path| {
            // Spotlight knows when it was last opened; the modification time
            // only knows when it was downloaded, which is never the question.
            let days = days_since_used(path).unwrap_or_else(|| fsx::age_days(path));
            if days < min_days {
                return None;
            }
            Some(Entry {
                name: format::tilde(path),
                path: path.clone(),
                bytes: fsx::size_of(path),
                unused_days: Some(days),
                evidence: None,
            })
        },
        |_, path| {
            let count = seen.fetch_add(1, Ordering::Relaxed) + 1;
            progress(count, path);
        },
    );

    let mut found: Vec<Entry> = found.into_iter().flatten().collect();
    found.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    found
}

/// Sum of the sizes of a list of entries.
pub fn total(entries: &[Entry]) -> u64 {
    entries.iter().map(|entry| entry.bytes).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_application_nothing_remembers_is_dated_from_its_install() {
        // The bug this pins: macOS lets `kMDItemLastUsedDate` lapse on
        // anything not opened recently, so Spotlight goes quiet on almost
        // every application worth reporting. Reading that silence as "never
        // opened" made an "unused for N days" filter match everything, at
        // any N. Silence now falls through to the application's own files,
        // and then to the age of the bundle — which is a ceiling: something
        // installed today cannot have been idle for a year.
        assert_eq!(
            resolve(None, || None, || 12),
            (12, Evidence::Install),
            "with nothing to go on, the install age is all that can be said"
        );
    }

    #[test]
    fn an_unknown_age_never_satisfies_a_dormancy_filter() {
        // The shape of the filter `apps --unused N` applies. What broke it
        // was an entry that could not be dated being kept rather than
        // dropped — with `is_none_or`, every such application matched every
        // threshold, including ones opened that morning.
        let dated = |days: Option<u64>| Entry {
            name: "Demo".to_string(),
            path: PathBuf::from("/Applications/Demo.app"),
            bytes: 0,
            unused_days: days,
            evidence: None,
        };

        let mut apps = vec![dated(Some(400)), dated(Some(3)), dated(None)];
        apps.retain(|app| app.unused_days.is_some_and(|used| used >= 180));

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].unused_days, Some(400));
    }

    #[test]
    fn the_strongest_signal_wins_and_the_rest_go_unread() {
        // Spotlight answered, so neither fallback should even be reached.
        assert_eq!(
            resolve(
                Some(4),
                || panic!("traces read anyway"),
                || panic!("age read anyway")
            ),
            (4, Evidence::Launch)
        );
        // It did not, so the files belonging to the application decide, and
        // the bundle's age stays out of it.
        assert_eq!(
            resolve(None, || Some(90), || panic!("age read anyway")),
            (90, Evidence::Traces)
        );
    }
}
