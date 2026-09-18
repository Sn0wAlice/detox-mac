//! Nettoyage Docker : conteneurs, images, caches de build et réseaux inutilisés.
//!
//! Les volumes ne sont jamais touchés : ils contiennent les données des
//! utilisateurs (bases, uploads…) et leur suppression n'est pas récupérable.

use crate::sys::cmd;

/// Commandes de purge exécutées, dans l'ordre.
const PRUNE: [&[&str]; 4] = [
    &["container", "prune", "-f"],
    &["image", "prune", "-a", "-f"],
    &["builder", "prune", "-a", "-f"],
    &["network", "prune", "-f"],
];

/// Vérifie que Docker est installé et que le démon répond.
pub fn availability() -> Result<(), String> {
    if !cmd::exists("docker") {
        return Err("Docker n'est pas installé".to_string());
    }
    match cmd::run("docker", &["version", "--format", "{{.Server.Version}}"]) {
        Ok(_) => Ok(()),
        Err(_) => Err("le démon Docker ne tourne pas".to_string()),
    }
}

/// Espace récupérable d'après `docker system df`.
#[derive(Debug, Default, Clone)]
pub struct Reclaimable {
    pub bytes: u64,
    /// Détail par type de ressource (images, conteneurs, cache de build).
    pub details: Vec<String>,
}

/// Interroge `docker system df` pour estimer l'espace récupérable.
pub fn reclaimable() -> Result<Reclaimable, String> {
    let output = cmd::run("docker", &["system", "df", "--format", "{{json .}}"])?;
    let mut result = Reclaimable::default();

    for line in output.stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = value["Type"].as_str().unwrap_or_default();
        // Les volumes ne sont pas purgés : on ne les compte pas non plus.
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

/// Résultat d'une purge.
pub struct Pruned {
    pub freed: u64,
    pub messages: Vec<String>,
}

/// Lance les purges Docker. Renvoie l'espace libéré et le détail des commandes.
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
            Err(err) => messages.push(format!("{label} — échec : {err}")),
        }
    }

    Ok(Pruned { freed, messages })
}

/// Commandes qui seraient lancées, pour le mode simulation.
pub fn planned_commands() -> Vec<String> {
    PRUNE
        .iter()
        .map(|args| format!("docker {}", args.join(" ")))
        .collect()
}

/// Extrait `Total reclaimed space: 1.2GB` de la sortie d'un `prune`.
fn reclaimed_space(output: &str) -> Option<u64> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("Total reclaimed space:"))
        .and_then(|value| parse_size(value.trim()))
}

/// Analyse une taille telle que Docker l'affiche : `1.093GB`, `972.5MB`, `0B`.
///
/// Docker utilise des unités décimales, contrairement au reste de l'outil.
fn parse_size(input: &str) -> Option<u64> {
    let text = input.trim();
    // `docker system df` renvoie « 1.2GB (57%) » : on ne garde que la taille.
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
        assert_eq!(parse_size("bientôt"), None);
    }

    #[test]
    fn reads_reclaimed_space_from_prune_output() {
        let output = "Deleted Containers:\nabc123\n\nTotal reclaimed space: 1.5GB";
        assert_eq!(reclaimed_space(output), Some(1_500_000_000));
        assert_eq!(reclaimed_space("rien à supprimer"), None);
    }

    #[test]
    fn volumes_are_never_pruned() {
        assert!(!PRUNE.iter().any(|args| args.contains(&"volume")));
    }
}
