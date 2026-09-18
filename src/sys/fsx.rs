//! Filesystem helpers.
//!
//! Two rules run through this module:
//!
//! * **Measure what the disk actually holds.** Sizes come from the allocated
//!   block count, not the apparent length, and a file reachable through
//!   several hard links is counted once.
//! * **Never fail silently.** A directory that cannot be read is recorded and
//!   reported, because "permission denied" must never be shown as "nothing to
//!   clean".

use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::config::Excludes;
use crate::format;

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

/// What a deletion actually does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Disposal {
    /// Move to the trash: recoverable, but space is only freed once the trash
    /// is emptied.
    Trash,
    /// Remove for good.
    Purge,
}

impl Disposal {
    pub fn is_trash(self) -> bool {
        self == Self::Trash
    }
}

/// How one removal run must behave.
#[derive(Debug, Clone, Copy)]
pub struct Policy<'a> {
    /// Measure only, change nothing.
    pub dry_run: bool,
    pub disposal: Disposal,
    /// Paths that must survive whatever happens.
    pub excludes: &'a Excludes,
}

impl<'a> Policy<'a> {
    pub fn new(dry_run: bool, disposal: Disposal, excludes: &'a Excludes) -> Self {
        Self {
            dry_run,
            disposal,
            excludes,
        }
    }

    /// A policy that always removes for good — for the trash itself, and for
    /// the throwaway files that trashing would only pile up.
    pub fn purging(self) -> Self {
        Self {
            disposal: Disposal::Purge,
            ..self
        }
    }
}

// ── measurement ─────────────────────────────────────────────────────────────

/// Bytes a file really occupies: allocated blocks, so sparse and transparently
/// compressed files are not overstated.
fn occupied(meta: &fs::Metadata) -> u64 {
    meta.blocks().saturating_mul(512)
}

/// Accumulates a measurement across one or more trees.
///
/// Reusing one `Sizer` over several directories is what makes hard-link
/// de-duplication work: a file linked from two of them is counted once.
#[derive(Debug, Default)]
pub struct Sizer {
    /// `(device, inode)` of the multiply-linked files already counted.
    counted: HashSet<(u64, u64)>,
    /// Paths that could not be read, and why.
    pub unreadable: Vec<String>,
    /// How many of those were a permission problem.
    pub denied: usize,
}

impl Sizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Size of an entry, recursive for a directory. Symlinks are never
    /// followed; only the link itself is counted.
    pub fn size_of(&mut self, path: &Path) -> u64 {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(err) => {
                self.note(path, &err);
                return 0;
            }
        };

        if !meta.is_dir() {
            return self.count_file(&meta);
        }

        let mut total = occupied(&meta);
        match fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    total += self.size_of(&entry.path());
                }
            }
            Err(err) => self.note(path, &err),
        }
        total
    }

    /// Counts a file once, however many hard links point at it.
    fn count_file(&mut self, meta: &fs::Metadata) -> u64 {
        if meta.nlink() > 1 && !self.counted.insert((meta.dev(), meta.ino())) {
            return 0;
        }
        occupied(meta)
    }

    fn note(&mut self, path: &Path, err: &io::Error) {
        if err.kind() == io::ErrorKind::PermissionDenied {
            self.denied += 1;
        }
        if self.unreadable.len() < 50 {
            self.unreadable
                .push(format!("{}: {err}", format::tilde(path)));
        }
    }
}

/// Size of a single entry, when no de-duplication across calls is needed.
pub fn size_of(path: &Path) -> u64 {
    Sizer::new().size_of(path)
}

/// Whether the process can read the parts of the home library that macOS
/// protects. Without it, whole targets silently measure zero.
pub fn has_full_disk_access() -> bool {
    // Reading this directory is exactly what the Full Disk Access grant
    // controls; it exists on every macOS install.
    let probe = home_join("Library/Safari");
    match fs::read_dir(&probe) {
        Ok(_) => true,
        Err(err) => err.kind() != io::ErrorKind::PermissionDenied,
    }
}

/// The sentence to show when directories could not be read.
pub fn full_disk_access_hint() -> &'static str {
    "grant Full Disk Access to your terminal in System Settings › Privacy & Security"
}

// ── removal ─────────────────────────────────────────────────────────────────

/// One entry moved to the trash, kept so the move can be undone.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
    pub bytes: u64,
}

/// Outcome of a deletion.
#[derive(Debug, Default, Clone)]
pub struct Removal {
    /// Bytes removed for good (or that would be, in dry-run mode).
    pub freed: u64,
    /// Bytes moved to the trash: recoverable, and not yet reclaimed.
    pub trashed: u64,
    /// Number of top-level entries removed.
    pub removed: usize,
    /// Entries left alone because the configuration protects them.
    pub excluded: usize,
    /// Entries that could not be removed.
    pub errors: Vec<String>,
    /// Trash moves, for the journal.
    pub moves: Vec<Move>,
}

impl Removal {
    /// Space the run accounted for, wherever it went.
    pub fn total(&self) -> u64 {
        self.freed + self.trashed
    }

    pub fn merge(&mut self, other: Removal) {
        self.freed += other.freed;
        self.trashed += other.trashed;
        self.removed += other.removed;
        self.excluded += other.excluded;
        self.errors.extend(other.errors);
        self.moves.extend(other.moves);
    }

    fn record(&mut self, path: &Path, bytes: u64, policy: &Policy) -> io::Result<()> {
        match policy.disposal {
            Disposal::Purge => {
                remove_for_good(path, policy.dry_run)?;
                self.freed += bytes;
            }
            Disposal::Trash => {
                let destination = move_to_trash(path, policy.dry_run)?;
                self.trashed += bytes;
                self.moves.push(Move {
                    from: path.to_path_buf(),
                    to: destination,
                    bytes,
                });
            }
        }
        self.removed += 1;
        Ok(())
    }
}

/// Removes an entry (file, link or directory) without following symlinks.
fn remove_for_good(path: &Path, dry_run: bool) -> io::Result<()> {
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

/// The trash that serves a given path: the user's own, or the one on the
/// volume the path lives on.
pub fn trash_dir_for(path: &Path) -> PathBuf {
    let components: Vec<_> = path.components().collect();
    // `/Volumes/<name>/…` has its own trash; moving across volumes would fail.
    if components.len() > 2 && path.starts_with("/Volumes") {
        let volume: PathBuf = components[..3].iter().collect();
        return volume.join(".Trashes").join(uid().to_string());
    }
    home_join(".Trash")
}

/// Numeric id of the current user, read from the ownership of its own home
/// directory — no libc binding needed for a single number.
pub fn uid() -> u32 {
    fs::metadata(home()).map(|meta| meta.uid()).unwrap_or(0)
}

/// Moves an entry to the trash, returning where it landed.
///
/// A name already taken in the trash is never overwritten.
pub fn move_to_trash(path: &Path, dry_run: bool) -> io::Result<PathBuf> {
    let trash = trash_dir_for(path);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("path has no name"))?;
    let destination = free_name(&trash, &name.to_string_lossy());

    if dry_run {
        return Ok(destination);
    }

    fs::create_dir_all(&trash)?;
    match fs::rename(path, &destination) {
        Ok(()) => Ok(destination),
        // EXDEV: the trash lives on another volume. Copying gigabytes to
        // "delete" them is never what the user meant, so say so instead.
        Err(err) if err.raw_os_error() == Some(18) => Err(io::Error::other(format!(
            "{} is on another volume than its trash — use --purge to remove it for good",
            format::tilde(path)
        ))),
        Err(err) => Err(err),
    }
}

/// A name that is still free inside `dir`.
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    for suffix in 2..1000 {
        let candidate = dir.join(format!("{name} {suffix}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{name} {}", std::process::id()))
}

/// Removes one entry, honouring the policy.
pub fn remove(path: &Path, policy: &Policy) -> Result<Removal, String> {
    let mut removal = Removal::default();

    if policy.excludes.blocks(path) {
        removal.excluded += 1;
        return Ok(removal);
    }

    let bytes = size_of(path);
    match removal.record(path, bytes, policy) {
        Ok(()) => Ok(removal),
        Err(err) => Err(format!("{}: {err}", format::tilde(path))),
    }
}

/// Empties a directory without removing the directory itself.
pub fn empty_dir(path: &Path, policy: &Policy) -> Removal {
    let mut result = Removal::default();

    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) => {
            result
                .errors
                .push(format!("{}: {err}", format::tilde(path)));
            return result;
        }
    };

    // One sizer for the whole directory, so hard-linked twins are not
    // counted twice (package stores are full of them).
    let mut sizer = Sizer::new();

    for entry in entries.flatten() {
        let entry_path = entry.path();

        if policy.excludes.blocks(&entry_path) {
            result.excluded += 1;
            continue;
        }

        let bytes = sizer.size_of(&entry_path);
        if let Err(err) = result.record(&entry_path, bytes, policy) {
            result
                .errors
                .push(format!("{}: {err}", format::tilde(&entry_path)));
        }
    }

    result.errors.extend(sizer.unreadable);
    result
}

// ── walking ─────────────────────────────────────────────────────────────────

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
/// skipped, and the depth is bounded. Returns what could not be read.
pub fn walk_files(root: &Path, max_depth: usize, visit: &mut impl FnMut(&Path, u64)) -> Sizer {
    fn inner(
        dir: &Path,
        depth: usize,
        max_depth: usize,
        sizer: &mut Sizer,
        visit: &mut impl FnMut(&Path, u64),
    ) {
        if depth > max_depth {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                sizer.note(dir, &err);
                return;
            }
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
                inner(&path, depth + 1, max_depth, sizer, visit);
            } else {
                let bytes = sizer.count_file(&meta);
                visit(&path, bytes);
            }
        }
    }

    let mut sizer = Sizer::new();
    inner(root, 0, max_depth, &mut sizer, visit);
    sizer
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

    fn policy(dry_run: bool, excludes: &Excludes) -> Policy<'_> {
        Policy::new(dry_run, Disposal::Purge, excludes)
    }

    #[test]
    fn measures_directory_recursively() {
        let dir = temp_dir("size");
        fs::write(dir.join("a.txt"), b"1234567890").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/b.txt"), b"12345").unwrap();

        // Block accounting, so the total is at least the bytes written and a
        // multiple of the block size — never the apparent 15 bytes.
        let measured = size_of(&dir);
        assert!(measured >= 15);
        assert_eq!(measured % 512, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn counts_a_hard_linked_file_once() {
        let dir = temp_dir("hardlink");
        let original = dir.join("a.bin");
        fs::write(&original, vec![0u8; 8192]).unwrap();
        fs::hard_link(&original, dir.join("b.bin")).unwrap();

        let twice = size_of(&dir.join("a.bin")) + size_of(&dir.join("b.bin"));
        let deduplicated = size_of(&dir);
        assert!(deduplicated < twice);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sparse_files_are_not_overstated() {
        let dir = temp_dir("sparse");
        let path = dir.join("hole.bin");
        let file = fs::File::create(&path).unwrap();
        // A one-gigabyte hole that occupies no blocks.
        file.set_len(1024 * 1024 * 1024).unwrap();
        drop(file);

        assert!(size_of(&path) < 1024 * 1024);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dry_run_keeps_files_but_reports_size() {
        let dir = temp_dir("dry-run");
        fs::write(dir.join("a.txt"), b"1234567890").unwrap();
        let excludes = Excludes::default();

        let result = empty_dir(&dir, &policy(true, &excludes));
        assert!(result.freed > 0);
        assert_eq!(result.removed, 1);
        assert!(dir.join("a.txt").exists());

        let result = empty_dir(&dir, &policy(false, &excludes));
        assert!(result.freed > 0);
        assert!(!dir.join("a.txt").exists());
        assert!(dir.exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn excluded_paths_survive() {
        let dir = temp_dir("excluded");
        fs::write(dir.join("keep.txt"), b"important").unwrap();
        fs::write(dir.join("drop.txt"), b"whatever").unwrap();
        let excludes = Excludes::new(vec!["keep.txt".to_string()]);

        let result = empty_dir(&dir, &policy(false, &excludes));
        assert_eq!(result.excluded, 1);
        assert_eq!(result.removed, 1);
        assert!(dir.join("keep.txt").exists());
        assert!(!dir.join("drop.txt").exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn trash_of_a_volume_is_on_that_volume() {
        let external = Path::new("/Volumes/Backup/old/file.txt");
        assert!(trash_dir_for(external).starts_with("/Volumes/Backup/.Trashes"));
        assert_eq!(trash_dir_for(&home_join("x")), home_join(".Trash"));
    }

    #[test]
    fn trashing_moves_instead_of_deleting() {
        let dir = temp_dir("trash-move");
        let victim = dir.join("gone.txt");
        fs::write(&victim, b"still here").unwrap();

        let excludes = Excludes::default();
        let policy = Policy::new(false, Disposal::Trash, &excludes);
        let result = empty_dir(&dir, &policy);

        assert_eq!(result.freed, 0, "trashing frees nothing yet");
        assert!(result.trashed > 0);
        assert_eq!(result.moves.len(), 1);
        assert!(!victim.exists());

        let landed = &result.moves[0].to;
        assert!(landed.exists());
        fs::remove_file(landed).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
