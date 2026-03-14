use std::fs;
use std::path::Path;

use super::utils::{format_size, is_dry_run, remove_file};

fn find_and_remove_dsstore(dir: &Path, count: &mut u64, freed: &mut u64) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".Trash" || name == "Library" || name == ".git" || name == "node_modules" {
                continue;
            }
            find_and_remove_dsstore(&path, count, freed);
        } else if entry.file_name() == ".DS_Store" {
            let size = path.metadata().map(|m| m.len()).unwrap_or(0);
            let _ = remove_file(&path);
            *count += 1;
            *freed += size;
        }
    }
}

pub fn clean_dsstore() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();
    let home_path = Path::new(&home);

    log.push("Recherche des fichiers .DS_Store...".to_string());

    let mut count = 0u64;
    let mut freed = 0u64;
    find_and_remove_dsstore(home_path, &mut count, &mut freed);

    if count == 0 {
        log.push("  Aucun fichier .DS_Store trouvé.".to_string());
    } else if is_dry_run() {
        log.push(format!(
            "  {} fichier(s) trouvé(s) ({} seraient libéré(s))",
            count,
            format_size(freed)
        ));
    } else {
        log.push(format!(
            "  {} fichier(s) supprimé(s) ({} libéré(s))",
            count,
            format_size(freed)
        ));
    }

    log
}
