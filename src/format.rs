//! Human-readable sizes, durations and paths.

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;
const TIB: u64 = 1024 * GIB;

/// Formats a byte count: `1.40 GB`, `860 MB`, `12 KB`…
pub fn size(bytes: u64) -> String {
    match bytes {
        b if b >= TIB => format!("{:.2} TB", b as f64 / TIB as f64),
        b if b >= GIB => format!("{:.2} GB", b as f64 / GIB as f64),
        b if b >= MIB => format!("{:.1} MB", b as f64 / MIB as f64),
        b if b >= KIB => format!("{:.0} KB", b as f64 / KIB as f64),
        b => format!("{b} B"),
    }
}

/// Parses a size given on the command line: `500M`, `1.5G`, `1024`, `200 KB`.
pub fn parse_size(input: &str) -> Result<u64, String> {
    let raw = input.trim().to_ascii_lowercase().replace(' ', "");
    let digits_end = raw
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(raw.len());
    let (number, unit) = raw.split_at(digits_end);

    let value: f64 = number
        .parse()
        .map_err(|_| format!("invalid size: `{input}`"))?;

    let multiplier = match unit.trim_end_matches('b') {
        "" => 1.0,
        "k" => KIB as f64,
        "m" => MIB as f64,
        "g" => GIB as f64,
        "t" => TIB as f64,
        other => return Err(format!("unknown unit: `{other}` (expected K, M, G or T)")),
    };

    Ok((value * multiplier) as u64)
}

/// Formats a `ps` elapsed time (`3-04:12:33`, `04:12:33`, `12:33`).
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
        (0, hours, minutes) => format!("{hours}h{minutes:02}"),
        (days, hours, _) => format!("{days}d {hours}h"),
    }
}

/// Shortens a path by replacing the home directory with `~`.
pub fn tilde(path: &std::path::Path) -> String {
    let display = path.display().to_string();
    let home = crate::sys::fsx::home().display().to_string();
    if !home.is_empty() && display.starts_with(&home) {
        format!("~{}", &display[home.len()..])
    } else {
        display
    }
}

/// Shortens text that overflows its column, with an ellipsis.
pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Truncates from the left, keeping the tail that identifies a directory.
pub fn truncate_start(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    let kept: String = text.chars().skip(count - width.saturating_sub(1)).collect();
    format!("…{kept}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_each_unit() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(2 * KIB), "2 KB");
        assert_eq!(size(3 * MIB + MIB / 2), "3.5 MB");
        assert_eq!(size(2 * GIB), "2.00 GB");
    }

    #[test]
    fn parses_suffixes() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("500M").unwrap(), 500 * MIB);
        assert_eq!(parse_size("1.5g").unwrap(), GIB + GIB / 2);
        assert_eq!(parse_size("200 KB").unwrap(), 200 * KIB);
        assert_eq!(parse_size("2gb").unwrap(), 2 * GIB);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_size("plenty").is_err());
        assert!(parse_size("12x").is_err());
    }

    #[test]
    fn formats_elapsed_times() {
        assert_eq!(elapsed("12:33"), "12 min");
        assert_eq!(elapsed("04:12:33"), "4h12");
        assert_eq!(elapsed("3-04:12:33"), "3d 4h");
        assert_eq!(elapsed("weird"), "weird");
    }

    #[test]
    fn truncates_only_when_needed() {
        assert_eq!(truncate("Docker", 10), "Docker");
        assert_eq!(truncate("at.obdev.littlesnitch.daemon", 12), "at.obdev.li…");
    }

    #[test]
    fn truncate_start_keeps_the_tail() {
        assert_eq!(truncate_start("~/dev/app", 20), "~/dev/app");
        assert_eq!(
            truncate_start("/very/long/path/to/project", 12),
            "…/to/project"
        );
    }
}
