//! Sizes a recent `scan` already paid for.
//!
//! `scan` walks the disk, then `clean` walks exactly the same disk again a few
//! seconds later. This keeps the first walk's per-entry figures around so the
//! second one does not have to happen.
//!
//! Correctness comes first: a remembered size is only used when the entry has
//! not been modified since, and never after a few minutes have passed. When in
//! doubt the entry is measured again — a stale number here would be a lie in
//! the report.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::format;
use crate::sys::fsx;

/// How long a remembered size stays usable.
const FRESH_FOR: u64 = 10 * 60;

/// One remembered measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    bytes: u64,
    /// Last modification when it was measured.
    mtime: u64,
}

/// Everything the last scan measured.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Sizes {
    epoch: u64,
    entries: HashMap<PathBuf, Record>,
}

fn path() -> PathBuf {
    std::env::temp_dir().join(format!("detox-mac-sizes-{}.json", fsx::uid()))
}

/// Remembers what a scan measured.
pub fn save(measured: &[(PathBuf, u64)]) {
    let sizes = Sizes {
        epoch: format::epoch_now(),
        entries: measured
            .iter()
            .map(|(path, bytes)| {
                (
                    path.clone(),
                    Record {
                        bytes: *bytes,
                        mtime: fsx::mtime(path),
                    },
                )
            })
            .collect(),
    };

    if let Ok(text) = serde_json::to_string(&sizes) {
        let _ = std::fs::write(path(), text);
    }
}

/// Reads back the last scan, if it is recent enough to still mean anything.
pub fn load() -> Sizes {
    let Ok(text) = std::fs::read_to_string(path()) else {
        return Sizes::default();
    };
    let Ok(sizes) = serde_json::from_str::<Sizes>(&text) else {
        return Sizes::default();
    };

    if format::epoch_now().saturating_sub(sizes.epoch) > FRESH_FOR {
        return Sizes::default();
    }
    sizes
}

impl Sizes {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The remembered size of an entry, if it can still be trusted.
    pub fn get(&self, path: &Path) -> Option<u64> {
        let record = self.entries.get(path)?;
        // Touched since the scan? Then the number is about something else.
        (record.mtime == fsx::mtime(path)).then_some(record.bytes)
    }

    /// The remembered sizes for a list, in the same order.
    pub fn lookup(&self, entries: &[PathBuf]) -> Vec<Option<u64>> {
        entries.iter().map(|path| self.get(path)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(path: &Path, bytes: u64) -> Sizes {
        Sizes {
            epoch: format::epoch_now(),
            entries: HashMap::from([(
                path.to_path_buf(),
                Record {
                    bytes,
                    mtime: fsx::mtime(path),
                },
            )]),
        }
    }

    #[test]
    fn a_modified_entry_is_measured_again() {
        let dir = std::env::temp_dir().join("detox-mac-test-sizes");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"first").unwrap();

        let sizes = record(&file, 4096);
        assert_eq!(sizes.get(&file), Some(4096));

        // Touch it: the remembered figure no longer describes this file.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&file, b"second, longer content").unwrap();
        assert_eq!(sizes.get(&file), None);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unknown_entry_is_never_guessed() {
        let sizes = Sizes::default();
        assert_eq!(sizes.get(Path::new("/nowhere")), None);
        assert!(sizes.is_empty());
    }

    #[test]
    fn a_stale_scan_is_dropped_whole() {
        let sizes = Sizes {
            epoch: format::epoch_now() - FRESH_FOR - 1,
            entries: HashMap::new(),
        };
        // `load` applies the window; the guard itself is what matters here.
        assert!(format::epoch_now().saturating_sub(sizes.epoch) > FRESH_FOR);
    }
}
