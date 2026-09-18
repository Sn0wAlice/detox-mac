//! Docker cleanup: unused containers, images, build caches and networks.
//!
//! Volumes are never touched: they hold user data (databases, uploads…) and
//! removing them cannot be undone.

use crate::sys::cmd;

/// Prune commands, in the order they run.
const PRUNE: [&[&str]; 4] = [
    &["container", "prune", "-f"],
    &["image", "prune", "-a", "-f"],
    &["builder", "prune", "-a", "-f"],
    &["network", "prune", "-f"],
];

/// Checks that Docker is installed and that the daemon answers.
pub fn availability() -> Result<(), String> {
    if !cmd::exists("docker") {
        return Err("Docker is not installed".to_string());
    }
    match cmd::run("docker", &["version", "--format", "{{.Server.Version}}"]) {
        Ok(_) => Ok(()),
        Err(_) => Err("the Docker daemon is not running".to_string()),
    }
}

/// Reclaimable space according to `docker system df`.
#[derive(Debug, Default, Clone)]
pub struct Reclaimable {
    pub bytes: u64,
    /// Breakdown per resource type (images, containers, build cache).
    pub details: Vec<String>,
}

/// Queries `docker system df` to estimate the reclaimable space.
pub fn reclaimable() -> Result<Reclaimable, String> {
    let output = cmd::run("docker", &["system", "df", "--format", "{{json .}}"])?;
    let mut result = Reclaimable::default();

    for line in output.stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = value["Type"].as_str().unwrap_or_default();
        // Volumes are never pruned, so they are not counted either.
        if kind.eq_ignore_ascii_case("Local Volumes") {
            continue;
        }

        let Some(bytes) = value["Reclaimable"].as_str().and_then(parse_size) else {
            continue;
        };
        if bytes > 0 {
            result.bytes += bytes;
            result
                .details
                .push(format!("{kind} : {}", crate::format::size(bytes)));
        }
    }

    Ok(result)
}

/// Result of a prune.
pub struct Pruned {
    pub freed: u64,
    pub messages: Vec<String>,
}

/// Runs the Docker prunes. Returns the freed space and a per-command detail.
pub fn prune() -> Result<Pruned, String> {
    let mut freed = 0;
    let mut messages = Vec::new();

    for args in PRUNE {
        let label = format!("docker {}", args[..2].join(" "));
        match cmd::run("docker", args) {
            Ok(output) => {
                let reclaimed = reclaimed_space(output.text()).unwrap_or(0);
                freed += reclaimed;
                messages.push(format!("{label} — {}", crate::format::size(reclaimed)));
            }
            Err(err) => messages.push(format!("{label} — failed: {err}")),
        }
    }

    Ok(Pruned { freed, messages })
}

/// Commands that would run, for dry-run mode.
pub fn planned_commands() -> Vec<String> {
    PRUNE
        .iter()
        .map(|args| format!("docker {}", args.join(" ")))
        .collect()
}

/// Extracts `Total reclaimed space: 1.2GB` from a `prune` output.
fn reclaimed_space(output: &str) -> Option<u64> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("Total reclaimed space:"))
        .and_then(|value| parse_size(value.trim()))
}

/// Parses a size the way Docker prints it: `1.093GB`, `972.5MB`, `0B`.
///
/// Docker uses decimal units, unlike the rest of this tool.
fn parse_size(input: &str) -> Option<u64> {
    let text = input.trim();
    // `docker system df` reports "1.2GB (57%)": keep the size only.
    let text = text.split_whitespace().next()?;

    let split = text.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (number, unit) = text.split_at(split);
    let value: f64 = number.parse().ok()?;

    let multiplier: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };

    Some((value * multiplier) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_sizes() {
        assert_eq!(parse_size("0B"), Some(0));
        assert_eq!(parse_size("972.5MB"), Some(972_500_000));
        assert_eq!(parse_size("1.093GB"), Some(1_093_000_000));
        assert_eq!(parse_size("2GiB"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("1.2GB (57%)"), Some(1_200_000_000));
        assert_eq!(parse_size("soon"), None);
    }

    #[test]
    fn reads_reclaimed_space_from_prune_output() {
        let output = "Deleted Containers:\nabc123\n\nTotal reclaimed space: 1.5GB";
        assert_eq!(reclaimed_space(output), Some(1_500_000_000));
        assert_eq!(reclaimed_space("nothing to delete"), None);
    }

    #[test]
    fn volumes_are_never_pruned() {
        assert!(!PRUNE.iter().any(|args| args.contains(&"volume")));
    }
}
