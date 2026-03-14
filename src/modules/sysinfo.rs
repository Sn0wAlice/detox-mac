use std::fs;
use std::process::Command;

use super::utils::{dir_size, format_size};

fn run_sysctl(key: &str) -> Option<String> {
    Command::new("sysctl")
        .args(["-n", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn count_plist_files(dir: &str) -> (u32, u32) {
    let mut apple = 0u32;
    let mut tiers = 0u32;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".plist") {
                if name.starts_with("com.apple.") {
                    apple += 1;
                } else {
                    tiers += 1;
                }
            }
        }
    }
    (apple, tiers)
}

pub fn get_system_info() -> Vec<String> {
    let mut log = vec![];
    let home = std::env::var("HOME").unwrap_or_default();

    // macOS version
    if let Ok(output) = Command::new("sw_vers").output() {
        let text = String::from_utf8_lossy(&output.stdout);
        let mut name = String::new();
        let mut version = String::new();
        for line in text.lines() {
            if line.starts_with("ProductName") {
                name = line.split(':').nth(1).unwrap_or("").trim().to_string();
            } else if line.starts_with("ProductVersion") {
                version = line.split(':').nth(1).unwrap_or("").trim().to_string();
            }
        }
        if !name.is_empty() {
            log.push(format!("OS: {} {}", name, version));
        }
    }

    // CPU
    if let Some(cpu) = run_sysctl("machdep.cpu.brand_string") {
        log.push(format!("CPU: {}", cpu));
    }
    if let Some(cores) = run_sysctl("hw.ncpu") {
        log.push(format!("  Coeurs: {}", cores));
    }

    // RAM
    if let Some(mem) = run_sysctl("hw.memsize") {
        if let Ok(bytes) = mem.parse::<u64>() {
            log.push(format!("RAM: {} totale", format_size(bytes)));
        }
    }

    if let Ok(output) = Command::new("vm_stat").output() {
        let text = String::from_utf8_lossy(&output.stdout);
        let ps = run_sysctl("hw.pagesize")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(16384);

        let mut free = 0u64;
        let mut active = 0u64;
        let mut inactive = 0u64;
        let mut wired = 0u64;
        let mut compressed = 0u64;

        for line in text.lines() {
            let val = line
                .split(':')
                .nth(1)
                .map(|v| v.trim().trim_end_matches('.'))
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);

            if line.starts_with("Pages free") {
                free = val * ps;
            } else if line.starts_with("Pages active") {
                active = val * ps;
            } else if line.starts_with("Pages inactive") {
                inactive = val * ps;
            } else if line.starts_with("Pages wired") {
                wired = val * ps;
            } else if line.contains("compressor") {
                compressed = val * ps;
            }
        }

        let used = active + wired + compressed;
        log.push(format!(
            "  Utilisée: {} | Libre: {} | Inactive: {}",
            format_size(used),
            format_size(free),
            format_size(inactive)
        ));
    }

    // Disque
    if let Ok(output) = Command::new("df").args(["-H", "/"]).output() {
        let text = String::from_utf8_lossy(&output.stdout);
        if let Some(line) = text.lines().nth(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 {
                log.push(format!(
                    "Disque: {} total | {} utilisé | {} libre ({})",
                    parts[1], parts[2], parts[3], parts[4]
                ));
            }
        }
    }

    // Uptime
    if let Ok(output) = Command::new("uptime").output() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if let Some(up_part) = text.split("up ").nth(1) {
            let uptime = up_part
                .split(',')
                .take(2)
                .collect::<Vec<_>>()
                .join(",")
                .trim()
                .to_string();
            log.push(format!("Uptime: {}", uptime));
        }
    }

    log.push(String::new());
    log.push("── Résumé nettoyage ──".to_string());

    // Tailles des zones nettoyables
    let cache_dir = format!("{}/Library/Caches", home);
    let cache_size = dir_size(std::path::Path::new(&cache_dir));
    let trash_dir = format!("{}/.Trash", home);
    let trash_size = dir_size(std::path::Path::new(&trash_dir));
    let logs_dir = format!("{}/Library/Logs", home);
    let logs_size = dir_size(std::path::Path::new(&logs_dir));

    log.push(format!(
        "Caches: {} | Corbeille: {} | Logs: {}",
        format_size(cache_size),
        format_size(trash_size),
        format_size(logs_size)
    ));

    let total = cache_size + trash_size + logs_size;
    log.push(format!("Espace récupérable: {}", format_size(total)));

    // Agents de démarrage
    let (a1, t1) = count_plist_files(&format!("{}/Library/LaunchAgents", home));
    let (a2, t2) = count_plist_files("/Library/LaunchAgents");
    let (a3, t3) = count_plist_files("/Library/LaunchDaemons");
    let total_apple = a1 + a2 + a3;
    let total_tiers = t1 + t2 + t3;
    log.push(format!(
        "Agents: {} Apple, {} tiers",
        total_apple, total_tiers
    ));

    log
}
