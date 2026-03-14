use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use super::utils::is_dry_run;

/// Récupère la liste des services chargés via launchctl
fn get_loaded_services() -> HashSet<String> {
    let mut set = HashSet::new();
    if let Ok(o) = Command::new("launchctl").arg("list").output() {
        let text = String::from_utf8_lossy(&o.stdout);
        for line in text.lines().skip(1) {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                set.insert(parts[2].to_string());
            }
        }
    }
    set
}

/// Extrait le label d'un fichier plist (nom du fichier sans .plist)
fn label_from_filename(filename: &str) -> &str {
    filename.strip_suffix(".plist").unwrap_or(filename)
}

fn is_apple(label: &str) -> bool {
    label.starts_with("com.apple.")
}

/// Liste détaillée avec label, statut actif/inactif, et tag Apple/Tiers
pub fn list_login_items_detailed() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();
    let loaded = get_loaded_services();

    let dirs = vec![
        ("LaunchAgents utilisateur", format!("{}/Library/LaunchAgents", home)),
        ("LaunchAgents système", "/Library/LaunchAgents".to_string()),
        ("LaunchDaemons système", "/Library/LaunchDaemons".to_string()),
    ];

    let mut total_apple = 0u32;
    let mut total_tiers = 0u32;

    for (section, dir) in dirs {
        log.push(format!("── {} ──", section));
        let path = Path::new(&dir);
        if !path.exists() {
            log.push("  (non trouvé)".to_string());
            continue;
        }

        let mut entries: Vec<_> = Vec::new();
        if let Ok(dir_entries) = fs::read_dir(path) {
            for entry in dir_entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".plist") {
                    entries.push(name);
                }
            }
        }
        entries.sort();

        if entries.is_empty() {
            log.push("  (vide)".to_string());
            continue;
        }

        for name in &entries {
            let label = label_from_filename(name);
            let apple = is_apple(label);
            let active = loaded.contains(label);
            let tag = if apple { "Apple" } else { "Tiers" };
            let status = if active { "actif" } else { "inactif" };

            if apple {
                total_apple += 1;
            } else {
                total_tiers += 1;
            }

            log.push(format!("  [{}] {} ({})", tag, label, status));
        }
    }

    log.push(String::new());
    log.push(format!(
        "Total: {} Apple, {} tiers",
        total_apple, total_tiers
    ));

    log
}

/// Liste uniquement les agents tiers (non-Apple)
pub fn list_third_party_agents() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();
    let loaded = get_loaded_services();

    let dirs = vec![
        ("LaunchAgents utilisateur", format!("{}/Library/LaunchAgents", home)),
        ("LaunchAgents système", "/Library/LaunchAgents".to_string()),
        ("LaunchDaemons système", "/Library/LaunchDaemons".to_string()),
    ];

    let mut count = 0u32;

    for (section, dir) in dirs {
        let path = Path::new(&dir);
        if !path.exists() {
            continue;
        }

        let mut found_in_section = false;
        if let Ok(dir_entries) = fs::read_dir(path) {
            let mut entries: Vec<String> = dir_entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.ends_with(".plist"))
                .collect();
            entries.sort();

            for name in &entries {
                let label = label_from_filename(name);
                if is_apple(label) {
                    continue;
                }
                if !found_in_section {
                    log.push(format!("── {} ──", section));
                    found_in_section = true;
                }
                let active = loaded.contains(label);
                let status = if active { "actif" } else { "inactif" };
                log.push(format!("  {} ({})", label, status));
                count += 1;
            }
        }
    }

    if count == 0 {
        log.push("Aucun agent tiers trouvé.".to_string());
    } else {
        log.push(format!("\n{} agent(s) tiers au total", count));
    }

    log
}

/// Désactive tous les agents tiers (launchctl unload)
pub fn disable_third_party_agents() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();
    let dry = is_dry_run();

    let dirs = vec![
        format!("{}/Library/LaunchAgents", home),
        "/Library/LaunchAgents".to_string(),
    ];

    let mut count = 0u32;

    for dir in dirs {
        let path = Path::new(&dir);
        if !path.exists() {
            continue;
        }

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".plist") {
                    continue;
                }
                let label = label_from_filename(&name);
                if is_apple(label) {
                    continue;
                }

                let plist_path = entry.path();
                if dry {
                    log.push(format!("  [SIMULATION] Désactivation: {}", label));
                } else {
                    log.push(format!("  Désactivation: {}", label));
                    let result = Command::new("launchctl")
                        .args(["unload", "-w"])
                        .arg(&plist_path)
                        .output();
                    match result {
                        Ok(o) if o.status.success() => {}
                        Ok(o) => {
                            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                            if !err.is_empty() {
                                log.push(format!("    Avertissement: {}", err));
                            }
                        }
                        Err(e) => log.push(format!("    Erreur: {}", e)),
                    }
                }
                count += 1;
            }
        }
    }

    if count == 0 {
        log.push("Aucun agent tiers à désactiver.".to_string());
    } else {
        let verb = if dry { "seraient désactivé(s)" } else { "désactivé(s)" };
        log.push(format!("{} agent(s) tiers {}.", count, verb));
    }

    log
}

/// Réactive tous les agents tiers (launchctl load)
pub fn enable_third_party_agents() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();
    let dry = is_dry_run();

    let dirs = vec![
        format!("{}/Library/LaunchAgents", home),
        "/Library/LaunchAgents".to_string(),
    ];

    let mut count = 0u32;

    for dir in dirs {
        let path = Path::new(&dir);
        if !path.exists() {
            continue;
        }

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".plist") {
                    continue;
                }
                let label = label_from_filename(&name);
                if is_apple(label) {
                    continue;
                }

                let plist_path = entry.path();
                if dry {
                    log.push(format!("  [SIMULATION] Réactivation: {}", label));
                } else {
                    log.push(format!("  Réactivation: {}", label));
                    let result = Command::new("launchctl")
                        .args(["load", "-w"])
                        .arg(&plist_path)
                        .output();
                    match result {
                        Ok(o) if o.status.success() => {}
                        Ok(o) => {
                            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                            if !err.is_empty() {
                                log.push(format!("    Avertissement: {}", err));
                            }
                        }
                        Err(e) => log.push(format!("    Erreur: {}", e)),
                    }
                }
                count += 1;
            }
        }
    }

    if count == 0 {
        log.push("Aucun agent tiers à réactiver.".to_string());
    } else {
        let verb = if dry { "seraient réactivé(s)" } else { "réactivé(s)" };
        log.push(format!("{} agent(s) tiers {}.", count, verb));
    }

    log
}
