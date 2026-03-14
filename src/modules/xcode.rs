use std::fs;
use std::path::Path;

use super::utils::{dir_size, format_size, is_dry_run, remove_dir_all, remove_file};

pub fn clean_xcode() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();

    let targets = vec![
        (
            format!("{}/Library/Developer/Xcode/DerivedData", home),
            "DerivedData (builds Xcode)",
        ),
        (
            format!("{}/Library/Developer/Xcode/iOS DeviceSupport", home),
            "iOS DeviceSupport",
        ),
        (
            format!("{}/Library/Developer/Xcode/watchOS DeviceSupport", home),
            "watchOS DeviceSupport",
        ),
        (
            format!("{}/Library/Developer/CoreSimulator/Caches", home),
            "Caches Simulateur",
        ),
        (
            format!("{}/Library/Developer/CoreSimulator/Devices", home),
            "Simulateurs (appareils)",
        ),
    ];

    let mut total_freed = 0u64;
    let mut found_any = false;

    for (dir, label) in targets {
        let path = Path::new(&dir);
        if !path.exists() {
            continue;
        }

        found_any = true;
        let size = dir_size(path);

        if size == 0 {
            log.push(format!("{}: vide", label));
            continue;
        }

        log.push(format!("{}: {}", label, format_size(size)));

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
        total_freed += size;
    }

    if !found_any {
        log.push("Xcode n'est pas installé ou aucun cache trouvé.".to_string());
    } else if is_dry_run() {
        log.push(format!("Total: {} seraient libéré(s)", format_size(total_freed)));
    } else {
        log.push(format!("Total libéré (Xcode): {}", format_size(total_freed)));
    }

    log
}
