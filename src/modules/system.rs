use std::process::Command;

use super::utils::is_dry_run;

pub fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

fn run_cmd(program: &str, args: &[&str]) -> Result<String, String> {
    match Command::new(program).args(args).output() {
        Ok(o) if o.status.success() => {
            Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            Err(if err.is_empty() {
                format!("Erreur (code {})", o.status.code().unwrap_or(-1))
            } else {
                err
            })
        }
        Err(e) => Err(e.to_string()),
    }
}

pub fn flush_dns() -> Vec<String> {
    let mut log = vec![];

    if !is_root() {
        log.push("Nécessite sudo. Relancez avec: sudo detox-mac".to_string());
        return log;
    }

    if is_dry_run() {
        log.push("[SIMULATION] Flush DNS".to_string());
        log.push("  dscacheutil -flushcache".to_string());
        log.push("  killall -HUP mDNSResponder".to_string());
        return log;
    }

    log.push("Flush du cache DNS...".to_string());
    match run_cmd("dscacheutil", &["-flushcache"]) {
        Ok(_) => log.push("  dscacheutil: OK".to_string()),
        Err(e) => log.push(format!("  dscacheutil: {}", e)),
    }
    match run_cmd("killall", &["-HUP", "mDNSResponder"]) {
        Ok(_) => log.push("  mDNSResponder: OK".to_string()),
        Err(e) => log.push(format!("  mDNSResponder: {}", e)),
    }
    log.push("Cache DNS vidé.".to_string());
    log
}

pub fn free_purgeable_space() -> Vec<String> {
    let mut log = vec![];

    if is_dry_run() {
        log.push("[SIMULATION] Libération espace purgeable".to_string());
        log.push("  tmutil thinlocalsnapshots / 999999999999 4".to_string());
        return log;
    }

    log.push("Libération de l'espace purgeable...".to_string());
    match run_cmd("tmutil", &["thinlocalsnapshots", "/", "999999999999", "4"]) {
        Ok(msg) => {
            log.push("  OK".to_string());
            if !msg.is_empty() {
                log.push(format!("  {}", msg));
            }
        }
        Err(e) => log.push(format!("  Erreur: {}", e)),
    }
    log
}

pub fn reindex_spotlight() -> Vec<String> {
    let mut log = vec![];

    if !is_root() {
        log.push("Nécessite sudo. Relancez avec: sudo detox-mac".to_string());
        return log;
    }

    if is_dry_run() {
        log.push("[SIMULATION] Réindexation Spotlight".to_string());
        log.push("  mdutil -E /".to_string());
        return log;
    }

    log.push("Réindexation de Spotlight...".to_string());
    match run_cmd("mdutil", &["-E", "/"]) {
        Ok(_) => log.push("  Index Spotlight réinitialisé.".to_string()),
        Err(e) => log.push(format!("  Erreur: {}", e)),
    }
    log
}

pub fn check_updates() -> Vec<String> {
    let mut log = vec![];
    log.push("Vérification des mises à jour macOS...".to_string());

    match Command::new("softwareupdate").args(["-l"]).output() {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let output = if !stderr.is_empty() { &stderr } else { &stdout };

            if output.contains("No new software available") || output.contains("No updates") {
                log.push("  Système à jour.".to_string());
            } else {
                for line in output.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty()
                        && !trimmed.starts_with("Software Update")
                        && !trimmed.starts_with("Finding")
                    {
                        log.push(format!("  {}", trimmed));
                    }
                }
            }
        }
        Err(e) => log.push(format!("  Erreur: {}", e)),
    }
    log
}

pub fn purge_ram() -> Vec<String> {
    let mut log = vec![];

    if !is_root() {
        log.push("Nécessite sudo. Relancez avec: sudo detox-mac".to_string());
        return log;
    }

    if is_dry_run() {
        log.push("[SIMULATION] Purge RAM".to_string());
        log.push("  purge".to_string());
        return log;
    }

    log.push("Purge de la RAM inactive...".to_string());
    match run_cmd("purge", &[]) {
        Ok(_) => log.push("  RAM purgée.".to_string()),
        Err(e) => log.push(format!("  Erreur: {}", e)),
    }
    log
}
