use std::fs;
use std::path::Path;

use super::utils::{dir_size, format_size, is_dry_run, remove_dir_all, remove_file};

pub fn clean_trash() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();
    let trash = format!("{}/.Trash", home);
    let path = Path::new(&trash);

    if path.exists() {
        let size = dir_size(path);
        log.push(format!("Corbeille: {}", format_size(size)));

        if size == 0 {
            log.push("  Corbeille déjà vide.".to_string());
            return log;
        }

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let _ = remove_dir_all(&p);
                } else {
                    let _ = remove_file(&p);
                }
            }
        }

        if is_dry_run() {
            log.push(format!("  {} seraient libéré(s)", format_size(size)));
        } else {
            log.push(format!("  {} libéré(s)", format_size(size)));
        }
    } else {
        log.push("Corbeille déjà vide.".to_string());
    }
    log
}

pub fn clean_all_trashes() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();
    let mut total_freed = 0u64;

    // Corbeille utilisateur
    let user_trash = format!("{}/.Trash", home);
    let ut_path = Path::new(&user_trash);
    if ut_path.exists() {
        let size = dir_size(ut_path);
        if size > 0 {
            log.push(format!("Corbeille utilisateur: {}", format_size(size)));
            if let Ok(entries) = fs::read_dir(ut_path) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        let _ = remove_dir_all(&p);
                    } else {
                        let _ = remove_file(&p);
                    }
                }
            }
            total_freed += size;
        }
    }

    // Corbeilles des volumes montés
    let volumes = Path::new("/Volumes");
    if let Ok(entries) = fs::read_dir(volumes) {
        for entry in entries.flatten() {
            let vol_path = entry.path();
            if !vol_path.is_dir() {
                continue;
            }
            let trash_path = vol_path.join(".Trashes");
            if trash_path.exists() {
                let size = dir_size(&trash_path);
                if size > 0 {
                    let vol_name = vol_path.file_name().unwrap_or_default().to_string_lossy();
                    log.push(format!("Corbeille [{}]: {}", vol_name, format_size(size)));
                    if let Ok(trash_entries) = fs::read_dir(&trash_path) {
                        for te in trash_entries.flatten() {
                            let p = te.path();
                            if p.is_dir() {
                                let _ = remove_dir_all(&p);
                            } else {
                                let _ = remove_file(&p);
                            }
                        }
                    }
                    total_freed += size;
                }
            }
        }
    }

    if total_freed == 0 {
        log.push("Toutes les corbeilles sont déjà vides.".to_string());
    } else if is_dry_run() {
        log.push(format!("Total: {} seraient libéré(s)", format_size(total_freed)));
    } else {
        log.push(format!("Total libéré (corbeilles): {}", format_size(total_freed)));
    }

    log
}
