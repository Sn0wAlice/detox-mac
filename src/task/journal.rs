//! Operation journal: what was removed, when, and how to put it back.
//!
//! Every run that removes something writes one file under
//! `~/.local/state/detox-mac/journal`. That file answers the only question
//! that matters after the fact — *what disappeared?* — and, when the entries
//! went to the trash rather than into the void, it is enough to undo the run.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::format;
use crate::sys::fsx::{self, Disposal, Move};

/// How many runs are kept before the oldest are dropped.
const KEEP: usize = 200;

/// One recorded run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    /// Sortable identifier, also the file name: `20260918-230512`.
    pub id: String,
    /// When it happened.
    pub epoch: u64,
    /// The command line that caused it, without the global options.
    pub command: String,
    /// What was done to the entries.
    pub disposal: Disposal,
    /// Bytes removed for good.
    pub freed: u64,
    /// Bytes moved to the trash.
    pub trashed: u64,
    /// Entries removed.
    pub removed: usize,
    /// Where each trashed entry landed, so it can come back.
    #[serde(default)]
    pub moves: Vec<Move>,
}

impl Run {
    /// Whether the run can still be undone in principle.
    pub fn reversible(&self) -> bool {
        self.disposal.is_trash() && !self.moves.is_empty()
    }
}

/// Directory holding the journal.
pub fn dir() -> PathBuf {
    match std::env::var_os("XDG_STATE_HOME") {
        Some(base) if !base.is_empty() => PathBuf::from(base).join("detox-mac/journal"),
        _ => fsx::home_join(".local/state/detox-mac/journal"),
    }
}

/// Records a run. Returns where it was written, or why it was not.
///
/// A run that changed nothing is not worth a file.
pub fn record(
    command: &str,
    disposal: Disposal,
    removal: &fsx::Removal,
) -> Result<Option<PathBuf>, String> {
    if removal.removed == 0 {
        return Ok(None);
    }

    let epoch = format::epoch_now();
    let run = Run {
        id: format::stamp(epoch),
        epoch,
        command: command.to_string(),
        disposal,
        freed: removal.freed,
        trashed: removal.trashed,
        removed: removal.removed,
        moves: removal.moves.clone(),
    };

    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", format::tilde(&dir)))?;

    let path = free_path(&dir, &run.id);
    let text =
        serde_json::to_string_pretty(&run).map_err(|err| format!("cannot serialise: {err}"))?;
    std::fs::write(&path, text).map_err(|err| format!("{}: {err}", format::tilde(&path)))?;

    prune(&dir);
    Ok(Some(path))
}

/// Two runs within the same second must not overwrite each other.
fn free_path(dir: &Path, id: &str) -> PathBuf {
    let candidate = dir.join(format!("{id}.json"));
    if !candidate.exists() {
        return candidate;
    }
    for suffix in 2..100 {
        let candidate = dir.join(format!("{id}-{suffix}.json"));
        if !candidate.exists() {
            return candidate;
        }
    }
    candidate
}

/// Recorded runs, most recent first.
pub fn list() -> Vec<Run> {
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return Vec::new();
    };

    let mut runs: Vec<Run> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();

    runs.sort_by(|a, b| b.epoch.cmp(&a.epoch).then_with(|| b.id.cmp(&a.id)));
    runs
}

/// Finds one run by identifier, or the most recent one.
pub fn find(id: Option<&str>) -> Option<Run> {
    let runs = list();
    match id {
        None => runs.into_iter().next(),
        Some(wanted) => runs.into_iter().find(|run| run.id == wanted),
    }
}

/// Drops the oldest files once there are too many.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();

    if files.len() <= KEEP {
        return;
    }
    files.sort();
    for old in &files[..files.len() - KEEP] {
        let _ = std::fs::remove_file(old);
    }
}

/// What an undo did.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Restored {
    pub restored: usize,
    pub bytes: u64,
    /// Entries whose original place is taken again — never overwritten.
    pub occupied: usize,
    /// Entries no longer in the trash: the user emptied it.
    pub gone: usize,
    pub errors: Vec<String>,
}

/// Puts a run's entries back where they came from.
///
/// Nothing is ever overwritten: an original path that exists again is left
/// alone and reported.
pub fn undo(run: &Run, dry_run: bool) -> Restored {
    let mut result = Restored::default();

    for entry in &run.moves {
        if !entry.to.exists() {
            result.gone += 1;
            continue;
        }
        if entry.from.exists() {
            result.occupied += 1;
            continue;
        }

        if dry_run {
            result.restored += 1;
            result.bytes += entry.bytes;
            continue;
        }

        if let Some(parent) = entry.from.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                result
                    .errors
                    .push(format!("{}: {err}", format::tilde(parent)));
                continue;
            }
        }

        match std::fs::rename(&entry.to, &entry.from) {
            Ok(()) => {
                result.restored += 1;
                result.bytes += entry.bytes;
            }
            Err(err) => result
                .errors
                .push(format!("{}: {err}", format::tilde(&entry.from))),
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_purged_run_cannot_be_undone() {
        let run = Run {
            id: "20260918-230512".to_string(),
            epoch: 0,
            command: "clean cache".to_string(),
            disposal: Disposal::Purge,
            freed: 10,
            trashed: 0,
            removed: 1,
            moves: Vec::new(),
        };
        assert!(!run.reversible());
    }

    #[test]
    fn undo_never_overwrites_what_came_back() {
        let dir = std::env::temp_dir().join("detox-mac-test-undo");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let from = dir.join("original.txt");
        let to = dir.join("trashed.txt");
        std::fs::write(&from, b"a new file took the place").unwrap();
        std::fs::write(&to, b"the old content").unwrap();

        let run = Run {
            id: "x".to_string(),
            epoch: 0,
            command: "clean cache".to_string(),
            disposal: Disposal::Trash,
            freed: 0,
            trashed: 10,
            removed: 1,
            moves: vec![Move {
                from: from.clone(),
                to: to.clone(),
                bytes: 10,
            }],
        };

        let result = undo(&run, false);
        assert_eq!(result.occupied, 1);
        assert_eq!(result.restored, 0);
        assert_eq!(
            std::fs::read_to_string(&from).unwrap(),
            "a new file took the place"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
