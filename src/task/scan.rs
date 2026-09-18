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
    /// Days since it was last opened, when Spotlight knows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unused_days: Option<u64>,
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
/// `None` when Spotlight has nothing on it — an unindexed volume, or a file
/// that has genuinely never been opened.
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
