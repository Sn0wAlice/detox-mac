//! Formatage et analyse des tailles lisibles par un humain.

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;
const TIB: u64 = 1024 * GIB;

/// Formate une taille en octets : `1.4 Go`, `860 Mo`, `12 Ko`…
pub fn size(bytes: u64) -> String {
    match bytes {
        b if b >= TIB => format!("{:.2} To", b as f64 / TIB as f64),
        b if b >= GIB => format!("{:.2} Go", b as f64 / GIB as f64),
        b if b >= MIB => format!("{:.1} Mo", b as f64 / MIB as f64),
        b if b >= KIB => format!("{:.0} Ko", b as f64 / KIB as f64),
        b => format!("{b} o"),
    }
}

/// Analyse une taille donnée en argument : `500M`, `1.5G`, `1024`, `200 Ko`.
pub fn parse_size(input: &str) -> Result<u64, String> {
    let raw = input.trim().to_ascii_lowercase().replace(' ', "");
    let digits_end = raw
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(raw.len());
    let (number, unit) = raw.split_at(digits_end);

    let value: f64 = number
        .parse()
        .map_err(|_| format!("taille invalide : « {input} »"))?;

    let multiplier = match unit.trim_end_matches('o').trim_end_matches('b') {
        "" => 1.0,
        "k" => KIB as f64,
        "m" => MIB as f64,
        "g" => GIB as f64,
        "t" => TIB as f64,
        other => {
            return Err(format!(
                "unité inconnue : « {other} » (attendu : K, M, G, T)"
            ));
        }
    };

    let bytes = value * multiplier;
    if bytes < 0.0 {
        return Err(format!("taille négative : « {input} »"));
    }
    Ok(bytes as u64)
}

/// Abrège un chemin en remplaçant le dossier personnel par `~`.
pub fn tilde(path: &std::path::Path) -> String {
    let display = path.display().to_string();
    let home = crate::sys::fsx::home().display().to_string();
    if !home.is_empty() && display.starts_with(&home) {
        format!("~{}", &display[home.len()..])
    } else {
        display
    }
}

/// Met en forme une durée `ps` (`3-04:12:33`, `04:12:33`, `12:33`).
pub fn elapsed(raw: &str) -> String {
    let (days, clock) = match raw.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().unwrap_or(0), clock),
        None => (0, raw),
    };

    let parts: Vec<u64> = clock
        .split(':')
        .map(|part| part.parse().unwrap_or(0))
        .collect();

    let (hours, minutes) = match parts.as_slice() {
        [hours, minutes, _] => (*hours, *minutes),
        [minutes, _] => (0, *minutes),
        _ => return raw.to_string(),
    };

    match (days, hours, minutes) {
        (0, 0, minutes) => format!("{minutes} min"),
        (0, hours, minutes) => format!("{hours} h {minutes:02}"),
        (days, hours, _) => format!("{days} j {hours} h"),
    }
}

/// Tronque un texte trop long pour sa colonne, avec une ellipse.
pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_each_unit() {
        assert_eq!(size(512), "512 o");
        assert_eq!(size(2 * KIB), "2 Ko");
        assert_eq!(size(3 * MIB + MIB / 2), "3.5 Mo");
        assert_eq!(size(2 * GIB), "2.00 Go");
    }

    #[test]
    fn parses_suffixes() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("500M").unwrap(), 500 * MIB);
        assert_eq!(parse_size("1.5g").unwrap(), GIB + GIB / 2);
        assert_eq!(parse_size("200 Ko").unwrap(), 200 * KIB);
        assert_eq!(parse_size("2gb").unwrap(), 2 * GIB);
    }

    #[test]
    fn truncates_only_when_needed() {
        assert_eq!(truncate("Docker", 10), "Docker");
        assert_eq!(truncate("at.obdev.littlesnitch.daemon", 12), "at.obdev.li…");
    }

    #[test]
    fn formats_elapsed_times() {
        assert_eq!(elapsed("12:33"), "12 min");
        assert_eq!(elapsed("04:12:33"), "4 h 12");
        assert_eq!(elapsed("3-04:12:33"), "3 j 4 h");
        assert_eq!(elapsed("bizarre"), "bizarre");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_size("beaucoup").is_err());
        assert!(parse_size("12x").is_err());
    }
}
