//! Collecte des informations système (macOS).

use serde::Serialize;

use super::cmd;

#[derive(Debug, Default, Clone, Serialize)]
pub struct Machine {
    pub os: String,
    pub build: String,
    pub host: String,
    pub cpu: String,
    pub cores: u32,
    pub memory: Memory,
    pub swap: Swap,
    pub disk: Option<Disk>,
    pub uptime: String,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Memory {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub inactive: u64,
}

/// Utilisation du fichier d'échange.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Swap {
    pub total: u64,
    pub used: u64,
}

impl Swap {
    /// Lit `vm.swapusage` : `total = 4096.00M  used = 2520.88M  free = 1575.12M`.
    pub fn collect() -> Self {
        let Some(raw) = cmd::sysctl("vm.swapusage") else {
            return Self::default();
        };

        let field = |key: &str| -> u64 {
            raw.split(key)
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(parse_swap_size)
                .unwrap_or(0)
        };

        Self {
            total: field("total ="),
            used: field("used ="),
        }
    }
}

/// Analyse une taille de `vm.swapusage` : `4096.00M`, `1.50G`.
fn parse_swap_size(input: &str) -> Option<u64> {
    let split = input.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (number, unit) = input.split_at(split);
    let value: f64 = number.parse().ok()?;

    let multiplier = match unit {
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };

    Some((value * multiplier) as u64)
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Disk {
    pub mount: String,
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub used_percent: u8,
}

impl Machine {
    /// Rassemble toutes les informations disponibles.
    pub fn collect() -> Self {
        let (os, build) = os_version();
        Self {
            os,
            build,
            host: cmd::run("scutil", &["--get", "ComputerName"])
                .map(|o| o.stdout)
                .unwrap_or_default(),
            cpu: cmd::sysctl("machdep.cpu.brand_string").unwrap_or_default(),
            cores: cmd::sysctl("hw.ncpu")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            memory: Memory::collect(),
            swap: Swap::collect(),
            disk: Disk::collect("/"),
            uptime: uptime(),
        }
    }
}

/// Nom + version de macOS, et numéro de build.
fn os_version() -> (String, String) {
    let Ok(output) = cmd::run("sw_vers", &[] as &[&str]) else {
        return (String::new(), String::new());
    };

    let (mut name, mut version, mut build) = (String::new(), String::new(), String::new());
    for line in output.stdout.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_string();
        match key.trim() {
            "ProductName" => name = value,
            "ProductVersion" => version = value,
            "BuildVersion" => build = value,
            _ => {}
        }
    }

    (format!("{name} {version}").trim().to_string(), build)
}

fn uptime() -> String {
    let Ok(output) = cmd::run("uptime", &[] as &[&str]) else {
        return String::new();
    };
    output
        .stdout
        .split("up ")
        .nth(1)
        .map(|rest| {
            rest.split(',')
                .take(2)
                .collect::<Vec<_>>()
                .join(",")
                .trim()
                .to_string()
        })
        .unwrap_or_default()
}

impl Memory {
    pub fn collect() -> Self {
        let total = cmd::sysctl("hw.memsize")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let page_size: u64 = cmd::sysctl("hw.pagesize")
            .and_then(|v| v.parse().ok())
            .unwrap_or(16384);

        let Ok(output) = cmd::run("vm_stat", &[] as &[&str]) else {
            return Self {
                total,
                ..Self::default()
            };
        };

        let (mut free, mut active, mut inactive, mut wired, mut compressed) = (0, 0, 0, 0, 0);
        for line in output.stdout.lines() {
            let Some((label, value)) = line.split_once(':') else {
                continue;
            };
            let pages: u64 = value
                .trim()
                .trim_end_matches('.')
                .parse()
                .unwrap_or_default();
            let bytes = pages * page_size;

            match label.trim() {
                "Pages free" => free = bytes,
                "Pages active" => active = bytes,
                "Pages inactive" => inactive = bytes,
                l if l.starts_with("Pages wired") => wired = bytes,
                l if l.contains("occupied by compressor") => compressed = bytes,
                _ => {}
            }
        }

        Self {
            total,
            used: active + wired + compressed,
            free,
            inactive,
        }
    }
}

impl Disk {
    fn collect(mount: &str) -> Option<Self> {
        // `df -k` renvoie des blocs de 1 Ko, indépendamment de la locale.
        let output = cmd::run("df", &["-k", mount]).ok()?;
        let line = output.stdout.lines().nth(1)?;
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            return None;
        }

        let kb = |index: usize| -> u64 { fields[index].parse::<u64>().unwrap_or(0) * 1024 };

        Some(Self {
            mount: fields.last().unwrap_or(&mount).to_string(),
            total: kb(1),
            used: kb(2),
            free: kb(3),
            used_percent: fields[4].trim_end_matches('%').parse().unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_swap_sizes() {
        assert_eq!(parse_swap_size("4096.00M"), Some(4096 * 1024 * 1024));
        assert_eq!(parse_swap_size("1.50G"), Some(1024 * 1024 * 1024 * 3 / 2));
        assert_eq!(parse_swap_size("0"), None);
    }
}
