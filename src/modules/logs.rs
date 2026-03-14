use std::fs;
use std::path::Path;

use super::utils::{dir_size, format_size, is_dry_run, remove_dir_all, remove_file};

fn clean_dir_contents(path: &Path) -> u64 {
    let mut cleaned = 0u64;

    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            let size = if p.is_dir() {
                let s = dir_size(&p);
                let _ = remove_dir_all(&p);
                s
            } else {
                let s = p.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = remove_file(&p);
                s
            };
            cleaned += size;
        }
    }

    cleaned
}

pub fn clean_logs() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = format!("{}/Library/Logs", home);
    let path = Path::new(&dir);

    if path.exists() {
        let size = dir_size(path);
        log.push(format!("Logs: {} ({})", dir, format_size(size)));

        if size == 0 {
            log.push("  Déjà vide.".to_string());
            return log;
        }

        let cleaned = clean_dir_contents(path);
        if is_dry_run() {
            log.push(format!("  {} seraient libéré(s)", format_size(cleaned)));
        } else {
            log.push(format!("  {} libéré(s)", format_size(cleaned)));
        }
    } else {
        log.push(format!("{} — absent", dir));
    }

    log
}
