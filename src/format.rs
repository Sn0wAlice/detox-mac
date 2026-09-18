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

/// Seconds since the Unix epoch, now.
pub fn epoch_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Offset of the local timezone, in seconds east of UTC.
///
/// Read once from `date`, which is the only thing on the system that already
/// knows the user's timezone rules.
fn local_offset() -> i64 {
    use std::sync::OnceLock;
    static OFFSET: OnceLock<i64> = OnceLock::new();

    *OFFSET.get_or_init(|| {
        let Ok(output) = crate::sys::cmd::run("date", &["+%z"]) else {
            return 0;
        };
        // `+0200`, `-0730`.
        let raw = output.stdout.trim();
        let (sign, digits) = match raw.strip_prefix('-') {
            Some(rest) => (-1, rest),
            None => (1, raw.trim_start_matches('+')),
        };
        if digits.len() < 4 {
            return 0;
        }
        let hours: i64 = digits[..2].parse().unwrap_or(0);
        let minutes: i64 = digits[2..4].parse().unwrap_or(0);
        sign * (hours * 3600 + minutes * 60)
    })
}

/// Splits a local timestamp into `(year, month, day, hour, minute, second)`.
fn civil(epoch: u64) -> (i64, u32, u32, u32, u32, u32) {
    let local = epoch as i64 + local_offset();
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);

    // Howard Hinnant's civil-from-days, shifted to a March-based year so that
    // the leap day lands at the end.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };

    (
        year,
        month,
        day,
        (seconds / 3600) as u32,
        (seconds % 3600 / 60) as u32,
        (seconds % 60) as u32,
    )
}

/// A timestamp a human reads: `2026-09-18 23:05`.
pub fn datetime(epoch: u64) -> String {
    let (year, month, day, hour, minute, _) = civil(epoch);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// A timestamp that sorts and makes a filename: `20260918-230512`.
pub fn stamp(epoch: u64) -> String {
    let (year, month, day, hour, minute, second) = civil(epoch);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

/// How long ago, in words: `3 days ago`, `just now`.
pub fn since(epoch: u64) -> String {
    let now = epoch_now();
    let seconds = now.saturating_sub(epoch);
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => {
            let days = seconds / 86_400;
            format!("{days} day{} ago", if days == 1 { "" } else { "s" })
        }
    }
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
