//! Filesystem helpers.
//!
//! Every destructive function takes a `dry_run` flag: in that mode it measures
//! exactly what would be deleted without touching anything.

use std::fs;
use std::path::{Path, PathBuf};

/// Home directory of the current user.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Path inside the home directory: `home_join("Library/Caches")`.
pub fn home_join(suffix: &str) -> PathBuf {
    home().join(suffix)
}

/// Size of an entry, recursive for a directory. Symlinks are never followed;
/// only the link itself is counted.
pub fn size_of(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }

    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            total += size_of(&entry.path());
        }
    }
    total
}

/// Outcome of a deletion.
#[derive(Debug, Default, Clone)]
pub struct Removal {
    /// Bytes freed (or that would be, in dry-run mode).
    pub freed: u64,
    /// Number of top-level entries removed.
    pub removed: usize,
    /// Entries that could not be removed.
    pub errors: Vec<String>,
}

impl Removal {
    pub fn merge(&mut self, other: Removal) {
        self.freed += other.freed;
        self.removed += other.removed;
        self.errors.extend(other.errors);
    }
}

/// Removes an entry (file, link or directory) without following symlinks.
pub fn remove(path: &Path, dry_run: bool) -> std::io::Result<()> {
    if dry_run {
        return Ok(());
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Empties a directory without removing the directory itself.
pub fn empty_dir(path: &Path, dry_run: bool) -> Removal {
    let mut result = Removal::default();

    let Ok(entries) = fs::read_dir(path) else {
        return result;
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        let size = size_of(&entry_path);
        match remove(&entry_path, dry_run) {
            Ok(()) => {
                result.freed += size;
                result.removed += 1;
            }
            Err(err) => result
                .errors
                .push(format!("{}: {err}", crate::format::tilde(&entry_path))),
        }
    }

    result
}

/// Directories skipped when walking the home tree.
const SKIPPED_DIRS: &[&str] = &[
    "Library",
    "node_modules",
    ".Trash",
    ".git",
    ".cargo",
    ".rustup",
    "Applications",
];

/// Walks `root` depth-first, calling `visit` for every file.
///
/// Symlinks are not followed, system and package-manager directories are
/// skipped, and the depth is bounded.
pub fn walk_files(root: &Path, max_depth: usize, visit: &mut impl FnMut(&Path, u64)) {
    fn inner(dir: &Path, depth: usize, max_depth: usize, visit: &mut impl FnMut(&Path, u64)) {
        if depth > max_depth {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata().or_else(|_| fs::symlink_metadata(&path)) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }

            if meta.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if SKIPPED_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                inner(&path, depth + 1, max_depth, visit);
            } else {
                visit(&path, meta.len());
            }
        }
    }

    inner(root, 0, max_depth, visit);
}

/// Lists the `.plist` files of a directory, sorted by name.
pub fn plists(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "plist"))
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("detox-mac-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn measures_directory_recursively() {
        let dir = temp_dir("size");
        fs::write(dir.join("a.txt"), b"1234567890").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/b.txt"), b"12345").unwrap();

        assert_eq!(size_of(&dir), 15);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dry_run_keeps_files_but_reports_size() {
        let dir = temp_dir("dry-run");
        fs::write(dir.join("a.txt"), b"1234567890").unwrap();

        let result = empty_dir(&dir, true);
        assert_eq!(result.freed, 10);
        assert_eq!(result.removed, 1);
        assert!(dir.join("a.txt").exists());

        let result = empty_dir(&dir, false);
        assert_eq!(result.freed, 10);
        assert!(!dir.join("a.txt").exists());
        assert!(dir.exists());

        fs::remove_dir_all(&dir).unwrap();
    }
}
