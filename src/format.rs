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
    fn rejects_garbage() {
        assert!(parse_size("beaucoup").is_err());
        assert!(parse_size("12x").is_err());
    }
}
