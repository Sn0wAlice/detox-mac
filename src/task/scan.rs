//! Disk inventory: applications and large files.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::format;
use crate::sys::fsx;

/// An item weighed on disk.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// Display name (application) or shortened path (file).
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
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
    }
}

/// Files bigger than `min_size` under `root`, largest first.
///
/// `progress` is called regularly with the number of files seen so far.
pub fn large_files(
    root: &Path,
    min_size: u64,
    max_depth: usize,
    progress: &mut impl FnMut(usize, &Path),
) -> Vec<Entry> {
    let mut files = Vec::new();
    let mut seen = 0usize;

    fsx::walk_files(root, max_depth, &mut |path, size| {
        seen += 1;
        progress(seen, path);

        if size >= min_size {
            files.push(Entry {
                name: format::tilde(path),
                path: path.to_path_buf(),
                bytes: size,
            });
        }
    });

    files.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    files
}

/// Sum of the sizes of a list of entries.
pub fn total(entries: &[Entry]) -> u64 {
    entries.iter().map(|entry| entry.bytes).sum()
}
