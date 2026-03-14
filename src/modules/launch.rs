use std::fs;
use std::process::Command;

use super::utils::{is_dry_run, remove_file};

fn unload_plist(path: &std::path::Path) -> Option<String> {
    if is_dry_run() {
        return None;
    }
    let result = Command::new("launchctl")
        .args(["unload", "-w"])
        .arg(path)
        .output();

    match result {
        Ok(o) if o.status.success() => None,
        Ok(o) => Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Some(e.to_string()),
    }
}

pub fn remove_user_launch_agents() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = format!("{}/Library/LaunchAgents", home);
    let dry = is_dry_run();
    let verb = if dry { "Serait supprimé" } else { "Suppression" };

    log.push(format!("Vérification: {}", dir));
    if let Ok(entries) = fs::read_dir(&dir) {
        let mut count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().map_or(false, |e| e == "plist") {
                continue;
            }

            log.push(format!("  Déchargement: {}", path.display()));
            if let Some(err) = unload_plist(&path) {
                log.push(format!("    Avertissement: {}", err));
            }

            log.push(format!("  {}: {}", verb, path.display()));
            if !dry {
                if let Err(e) = remove_file(&path) {
                    log.push(format!("    Erreur: {}", e));
                    continue;
                }
            }
            count += 1;
        }
        log.push(format!("{} agent(s) utilisateur {}.", count, if dry { "trouvé(s)" } else { "supprimé(s)" }));
    } else {
        log.push("  Aucun LaunchAgent utilisateur trouvé.".to_string());
    }
    log
}

pub fn remove_all_launch_items() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();
    let dry = is_dry_run();
    let verb = if dry { "Serait supprimé" } else { "Suppression" };

    let dirs = vec![
        format!("{}/Library/LaunchAgents", home),
        "/Library/LaunchAgents".into(),
        "/Library/LaunchDaemons".into(),
    ];

    for dir in dirs {
        log.push(format!("Vérification: {}", dir));
        if let Ok(entries) = fs::read_dir(&dir) {
            let mut count = 0;
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.extension().map_or(false, |e| e == "plist") {
                    continue;
                }

                log.push(format!("  Déchargement: {}", path.display()));
                if let Some(err) = unload_plist(&path) {
                    log.push(format!("    Avertissement: {}", err));
                }

                log.push(format!("  {}: {}", verb, path.display()));
                if !dry {
                    if let Err(e) = remove_file(&path) {
                        log.push(format!("    Erreur: {}", e));
                        continue;
                    }
                }
                count += 1;
            }
            log.push(format!("  {} {} dans {}", count, if dry { "trouvé(s)" } else { "supprimé(s)" }, dir));
        }
    }
    log
}
