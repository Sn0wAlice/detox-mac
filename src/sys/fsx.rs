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
use std::sync::{Arc, Mutex};

use crate::config::Excludes;
use crate::format;
use crate::sys::par;

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

/// `SF_DATALESS`: the file belongs to iCloud and its content is not on this
/// disk. Only macOS has the flag.
#[cfg(target_os = "macos")]
fn has_dataless_flag(meta: &fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt as MacMetadataExt;
    const SF_DATALESS: u32 = 0x4000_0000;
    meta.st_flags() & SF_DATALESS != 0
}

#[cfg(not(target_os = "macos"))]
fn has_dataless_flag(_meta: &fs::Metadata) -> bool {
    false
}

/// Whether a path is an iCloud placeholder rather than a real file.
///
/// It takes no space, so counting it would inflate every total — and deleting
/// it would remove the real file from iCloud, on every device.
pub fn is_evicted(path: &Path, meta: &fs::Metadata) -> bool {
    if has_dataless_flag(meta) {
        return true;
    }
    // The visible placeholder of an evicted file: `.name.ext.icloud`.
    path.extension().is_some_and(|ext| ext == "icloud")
        && path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

/// Accumulates a measurement across one or more trees.
///
/// The set of already-counted inodes is shared, which is what makes hard-link
/// de-duplication hold across directories *and* across threads.
#[derive(Debug, Default)]
pub struct Sizer {
    /// `(device, inode)` of the multiply-linked files already counted.
    counted: Arc<Mutex<HashSet<(u64, u64)>>>,
    /// Paths that could not be read, and why.
    pub unreadable: Vec<String>,
    /// How many of those were a permission problem.
    pub denied: usize,
    /// Files that live in iCloud and not on this disk.
    pub evicted: usize,
}

impl Sizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// A sizer for another thread, sharing what has already been counted.
    fn fork(&self) -> Self {
        Self {
            counted: Arc::clone(&self.counted),
            ..Self::default()
        }
    }

    /// Folds a forked sizer's findings back in.
    fn absorb(&mut self, other: Sizer) {
        self.denied += other.denied;
        self.evicted += other.evicted;
        for line in other.unreadable {
            if self.unreadable.len() < 50 {
                self.unreadable.push(line);
            }
        }
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
            return self.count_file(path, &meta);
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

    /// Counts a file once, however many hard links point at it, and never
    /// counts one that is not really here.
    fn count_file(&mut self, path: &Path, meta: &fs::Metadata) -> u64 {
        if is_evicted(path, meta) {
            self.evicted += 1;
            return 0;
        }
        if meta.nlink() > 1 {
            let mut counted = match self.counted.lock() {
                Ok(counted) => counted,
                Err(poisoned) => poisoned.into_inner(),
            };
            if !counted.insert((meta.dev(), meta.ino())) {
                return 0;
            }
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

/// Measures one or more trees at once, on a few threads.
///
/// Passing every directory of a target in one call is what keeps hard-link
/// de-duplication honest: a file linked from two package stores is counted
/// once, not once per store.
pub fn measure_all(roots: &[PathBuf]) -> (u64, Sizer) {
    let (totals, sizer) = measure_each(roots);
    (totals.iter().sum(), sizer)
}

/// Measures several trees and reports each one separately.
///
/// One shared set of counted inodes across the whole call, so the per-entry
/// figures still add up to a total that does not count a hard-linked file
/// twice.
pub fn measure_each(roots: &[PathBuf]) -> (Vec<u64>, Sizer) {
    let mut sizer = Sizer::new();
    let count = roots.len();
    let seed: Vec<(PathBuf, usize)> = roots
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, path)| (path, index))
        .collect();

    let states = par::drain(
        seed,
        || (sizer.fork(), vec![0u64; count]),
        |(worker, totals), (path, root), extra| {
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(err) => {
                    worker.note(&path, &err);
                    return;
                }
            };

            if !meta.is_dir() {
                totals[root] += worker.count_file(&path, &meta);
                return;
            }

            totals[root] += occupied(&meta);
            match fs::read_dir(&path) {
                Ok(entries) => extra.extend(entries.flatten().map(|entry| (entry.path(), root))),
                Err(err) => worker.note(&path, &err),
            }
        },
    );

    let mut totals = vec![0u64; count];
    for (worker, partial) in states {
        for (index, bytes) in partial.into_iter().enumerate() {
            totals[index] += bytes;
        }
        sizer.absorb(worker);
    }
    (totals, sizer)
}

/// Measures a single tree.
pub fn measure(path: &Path) -> (u64, Sizer) {
    measure_all(std::slice::from_ref(&path.to_path_buf()))
}

/// Last modification of an entry, in seconds since the epoch.
pub fn mtime(path: &Path) -> u64 {
    fs::symlink_metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Size of a single entry, when the caller has nothing to report.
pub fn size_of(path: &Path) -> u64 {
    measure(path).0
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

/// Removes a known list of entries, honouring the policy.
///
/// The caller picks what goes, which is what lets a measurement and the
/// removal that follows agree on exactly the same set.
pub fn remove_entries(entries: &[PathBuf], policy: &Policy) -> Removal {
    remove_measured(entries, &[], policy)
}

/// The same, with sizes somebody already paid to compute.
///
/// `known` is parallel to `entries`; a `None` is measured here. It is what
/// lets a `clean` right after a `scan` avoid walking the same disk twice.
pub fn remove_measured(entries: &[PathBuf], known: &[Option<u64>], policy: &Policy) -> Removal {
    let mut result = Removal::default();

    let wanted: Vec<(usize, &PathBuf)> = entries
        .iter()
        .enumerate()
        .filter(|(_, path)| {
            if policy.excludes.blocks(path) {
                result.excluded += 1;
                return false;
            }
            true
        })
        .collect();

    // Anything without a size yet is measured in one parallel pass, so
    // hard-linked twins are still only counted once.
    let unknown: Vec<PathBuf> = wanted
        .iter()
        .filter(|(index, _)| known.get(*index).copied().flatten().is_none())
        .map(|(_, path)| (*path).clone())
        .collect();
    let (measured, sizer) = measure_each(&unknown);
    result.errors.extend(sizer.unreadable);

    let mut next = 0;
    for (index, path) in wanted {
        let bytes = match known.get(index).copied().flatten() {
            Some(bytes) => bytes,
            None => {
                let bytes = measured.get(next).copied().unwrap_or(0);
                next += 1;
                bytes
            }
        };

        if let Err(err) = result.record(path, bytes, policy) {
            result.errors.push(err.to_string());
        }
    }

    result
}

/// The entries of a directory, oldest change first.
pub fn entries_of(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("{}: {err}", format::tilde(dir)))?;
    Ok(entries.flatten().map(|entry| entry.path()).collect())
}

/// Days since an entry last changed, as far as the filesystem knows.
pub fn age_days(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    let Ok(modified) = meta.modified() else {
        return 0;
    };
    modified
        .elapsed()
        .map(|since| since.as_secs() / 86_400)
        .unwrap_or(0)
}

// ── walking ─────────────────────────────────────────────────────────────────

/// Walks `root`, calling `visit` for every file with the space it occupies.
///
/// Symlinks are not followed, system and package-manager directories are
/// skipped, and the depth is bounded. The walk is threaded, so `visit` must be
/// callable from any of them. Returns what could not be read.
pub fn walk_files(root: &Path, max_depth: usize, visit: &(impl Fn(&Path, u64) + Sync)) -> Sizer {
    let mut sizer = Sizer::new();

    let states = par::drain(
        vec![(root.to_path_buf(), 0usize)],
        || sizer.fork(),
        |worker, (path, depth), extra| {
            let Ok(meta) = fs::symlink_metadata(&path) else {
                return;
            };
            if meta.file_type().is_symlink() {
                return;
            }

            if !meta.is_dir() {
                let bytes = worker.count_file(&path, &meta);
                visit(&path, bytes);
                return;
            }

            if depth > max_depth {
                return;
            }
            // The root is walked whatever it is called; only what is found
            // inside it can be skipped by name.
            let skip = depth > 0
                && path
                    .file_name()
                    .is_some_and(|name| SKIPPED_DIRS.contains(&name.to_string_lossy().as_ref()));
            if skip {
                return;
            }

            match fs::read_dir(&path) {
                Ok(entries) => {
                    extra.extend(entries.flatten().map(|entry| (entry.path(), depth + 1)))
                }
                Err(err) => worker.note(&path, &err),
            }
        },
    );

    for worker in states {
        sizer.absorb(worker);
    }
    sizer
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

        let entries = entries_of(&dir).unwrap();
        let result = remove_entries(&entries, &policy(true, &excludes));
        assert!(result.freed > 0);
        assert_eq!(result.removed, 1);
        assert!(dir.join("a.txt").exists());

        let result = remove_entries(&entries, &policy(false, &excludes));
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

        let result = remove_entries(&entries_of(&dir).unwrap(), &policy(false, &excludes));
        assert_eq!(result.excluded, 1);
        assert_eq!(result.removed, 1);
        assert!(dir.join("keep.txt").exists());
        assert!(!dir.join("drop.txt").exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_fresh_file_has_no_age() {
        let dir = temp_dir("age");
        let path = dir.join("new.txt");
        fs::write(&path, b"just written").unwrap();
        assert_eq!(age_days(&path), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn icloud_placeholders_are_recognised() {
        let dir = temp_dir("icloud");
        // The visible stand-in for an evicted file takes no space itself.
        let placeholder = dir.join(".report.pdf.icloud");
        fs::write(&placeholder, b"stub").unwrap();
        let meta = fs::symlink_metadata(&placeholder).unwrap();
        assert!(is_evicted(&placeholder, &meta));

        let real = dir.join("report.pdf");
        fs::write(&real, b"content").unwrap();
        let meta = fs::symlink_metadata(&real).unwrap();
        assert!(!is_evicted(&real, &meta));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn evicted_files_are_not_counted_as_space() {
        let dir = temp_dir("icloud-size");
        fs::write(dir.join(".big.mov.icloud"), vec![0u8; 4096]).unwrap();

        let (bytes, sizer) = measure(&dir);
        assert_eq!(sizer.evicted, 1);
        // Only the directory itself weighs anything.
        assert!(bytes < 4096);

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
        let result = remove_entries(&entries_of(&dir).unwrap(), &policy);

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
