use std::fs;
use std::path::Path;

use super::utils::{dir_size, format_size};

pub fn list_large_apps() -> Vec<String> {
    let mut log = vec![];
    let apps_dir = Path::new("/Applications");

    if !apps_dir.exists() {
        log.push("/Applications introuvable.".to_string());
        return log;
    }

    log.push("Scan de /Applications...".to_string());

    let mut apps: Vec<(String, u64)> = Vec::new();

    if let Ok(entries) = fs::read_dir(apps_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "app") {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let size = dir_size(&path);
                apps.push((name, size));
            }
        }
    }

    // Trier par taille décroissante
    apps.sort_by(|a, b| b.1.cmp(&a.1));

    // Afficher le top 15
    let total: u64 = apps.iter().map(|(_, s)| *s).sum();
    log.push(format!(
        "{} apps trouvées ({})",
        apps.len(),
        format_size(total)
    ));
    log.push(String::new());

    for (name, size) in apps.iter().take(15) {
        log.push(format!("  {:>10}  {}", format_size(*size), name));
    }

    if apps.len() > 15 {
        log.push(format!("  ... et {} autres", apps.len() - 15));
    }

    log
}

pub fn find_large_files() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();

    const MIN_SIZE: u64 = 500 * 1024 * 1024; // 500 MB

    log.push(format!(
        "Recherche des fichiers > {} dans ~...",
        format_size(MIN_SIZE)
    ));

    let mut large_files: Vec<(String, u64)> = Vec::new();

    scan_for_large_files(Path::new(&home), MIN_SIZE, &mut large_files, 0);

    // Trier par taille décroissante
    large_files.sort_by(|a, b| b.1.cmp(&a.1));

    if large_files.is_empty() {
        log.push("  Aucun fichier volumineux trouvé.".to_string());
    } else {
        log.push(format!("{} fichier(s) trouvé(s):", large_files.len()));
        log.push(String::new());

        let home_prefix = format!("{}/", home);
        for (path, size) in large_files.iter().take(20) {
            let display = path
                .strip_prefix(&home_prefix)
                .map(|p| format!("~/{}", p))
                .unwrap_or_else(|| path.clone());
            log.push(format!("  {:>10}  {}", format_size(*size), display));
        }

        if large_files.len() > 20 {
            log.push(format!("  ... et {} autres", large_files.len() - 20));
        }

        let total: u64 = large_files.iter().map(|(_, s)| *s).sum();
        log.push(format!("Total: {}", format_size(total)));
    }

    log
}

fn scan_for_large_files(dir: &Path, min_size: u64, results: &mut Vec<(String, u64)>, depth: u32) {
    // Limiter la profondeur pour éviter les scans trop longs
    if depth > 8 {
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip les dossiers problématiques
        if name.starts_with('.')
            || name == "Library"
            || name == "node_modules"
            || name == ".Trash"
        {
            continue;
        }

        if path.is_dir() {
            scan_for_large_files(&path, min_size, results, depth + 1);
        } else if let Ok(meta) = path.metadata() {
            if meta.len() >= min_size {
                results.push((path.to_string_lossy().to_string(), meta.len()));
            }
        }
    }
}
