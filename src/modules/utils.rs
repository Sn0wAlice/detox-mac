use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

static DRY_RUN: AtomicBool = AtomicBool::new(false);

pub fn set_dry_run(enabled: bool) {
    DRY_RUN.store(enabled, Ordering::Relaxed);
}

pub fn is_dry_run() -> bool {
    DRY_RUN.load(Ordering::Relaxed)
}

/// Supprime un fichier, sauf en mode dry-run
pub fn remove_file(path: &Path) -> std::io::Result<()> {
    if is_dry_run() {
        return Ok(());
    }
    fs::remove_file(path)
}

/// Supprime un dossier récursivement, sauf en mode dry-run
pub fn remove_dir_all(path: &Path) -> std::io::Result<()> {
    if is_dry_run() {
        return Ok(());
    }
    fs::remove_dir_all(path)
}

/// Calcule la taille totale d'un répertoire récursivement (en octets)
pub fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(meta) = p.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// Formate une taille en octets vers une chaîne lisible (Ko, Mo, Go)
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.2} Go", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} Mo", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} Ko", bytes as f64 / KB as f64)
    } else {
        format!("{} octets", bytes)
    }
}
