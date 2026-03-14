use std::process::Command;

use super::utils::is_dry_run;

pub fn clean_homebrew() -> Vec<String> {
    let mut log = vec![];

    let brew_path = Command::new("which").arg("brew").output();
    match brew_path {
        Ok(o) if o.status.success() => {}
        _ => {
            log.push("Homebrew n'est pas installé — ignoré.".to_string());
            return log;
        }
    }

    log.push("Nettoyage du cache Homebrew...".to_string());

    if let Ok(o) = Command::new("brew").args(["--cache"]).output() {
        let cache_dir = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if !cache_dir.is_empty() {
            let path = std::path::Path::new(&cache_dir);
            if path.exists() {
                let size = super::utils::dir_size(path);
                log.push(format!("  Cache Homebrew: {}", super::utils::format_size(size)));
            }
        }
    }

    if is_dry_run() {
        log.push("  [SIMULATION] brew cleanup --prune=all -s".to_string());
        return log;
    }

    match Command::new("brew")
        .args(["cleanup", "--prune=all", "-s"])
        .output()
    {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !stdout.is_empty() {
                for line in stdout.lines().take(10) {
                    log.push(format!("  {}", line));
                }
            }
            log.push("  Nettoyage Homebrew terminé.".to_string());
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            log.push(format!("  Erreur: {}", err));
        }
        Err(e) => {
            log.push(format!("  Erreur: {}", e));
        }
    }

    log
}
