use std::fs;
use std::path::Path;

use super::utils::{dir_size, format_size, is_dry_run, remove_dir_all, remove_file};

pub fn clean_cache() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = format!("{}/Library/Caches", home);
    let path = Path::new(&dir);

    if path.exists() {
        let size_before = dir_size(path);
        log.push(format!("Nettoyage: {} ({})", dir, format_size(size_before)));

        if let Ok(entries) = fs::read_dir(path) {
            let mut cleaned = 0u64;
            for entry in entries.flatten() {
                let entry_path = entry.path();
                let entry_size = if entry_path.is_dir() {
                    let s = dir_size(&entry_path);
                    let _ = remove_dir_all(&entry_path);
                    s
                } else {
                    let s = entry_path.metadata().map(|m| m.len()).unwrap_or(0);
                    let _ = remove_file(&entry_path);
                    s
                };
                cleaned += entry_size;
            }
            if is_dry_run() {
                log.push(format!("  {} seraient libéré(s)", format_size(cleaned)));
            } else {
                log.push(format!("  {} libéré(s)", format_size(cleaned)));
            }
        }
    } else {
        log.push(format!("{} — absent, rien à nettoyer", dir));
    }

    log
}
