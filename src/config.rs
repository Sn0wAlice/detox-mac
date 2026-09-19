//! User configuration: exclusions, disposal mode, defaults.
//!
//! Read from `~/.config/detox-mac/config.toml` (or `$DETOX_MAC_CONFIG`). The
//! format is a deliberately small subset of TOML — string, boolean and string
//! array assignments at the top level — so that the tool keeps its four
//! dependencies. Anything it cannot read is reported, never guessed.

use std::path::{Path, PathBuf};

use crate::sys::fsx::{self, Disposal};

/// Everything the user can set once and forget.
#[derive(Debug, Clone)]
pub struct Config {
    /// Paths that must never be touched.
    pub exclude: Excludes,
    /// What a deletion does by default.
    pub disposal: Disposal,
    /// Targets used by `clean` and `scan` when none is given.
    pub default_targets: Vec<String>,
    /// Ask a second time before anything irreversible.
    pub confirm_twice: bool,
    /// Leave alone anything touched more recently than this many days.
    pub min_age_days: u64,
    /// Where the settings were read from.
    pub source: Option<PathBuf>,
    /// Lines that could not be understood.
    pub problems: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            exclude: Excludes::default(),
            disposal: Disposal::Trash,
            default_targets: Vec::new(),
            confirm_twice: true,
            min_age_days: 0,
            source: None,
            problems: Vec::new(),
        }
    }
}

impl Config {
    /// Path of the configuration file, whether or not it exists.
    pub fn path() -> PathBuf {
        if let Some(explicit) = std::env::var_os("DETOX_MAC_CONFIG") {
            return PathBuf::from(explicit);
        }
        match std::env::var_os("XDG_CONFIG_HOME") {
            Some(base) if !base.is_empty() => PathBuf::from(base).join("detox-mac/config.toml"),
            _ => fsx::home_join(".config/detox-mac/config.toml"),
        }
    }

    /// Loads the configuration, falling back to the defaults.
    pub fn load() -> Self {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let mut config = Self::parse(&text);
        config.source = Some(path);
        config
    }

    fn parse(text: &str) -> Self {
        let mut config = Self::default();

        for (key, value, line) in entries(text) {
            match (key.as_str(), value) {
                ("exclude", Value::List(items)) => config.exclude = Excludes::new(items),
                ("default_targets", Value::List(items)) => config.default_targets = items,
                ("disposal", Value::Text(word)) => match word.as_str() {
                    "trash" => config.disposal = Disposal::Trash,
                    "purge" => config.disposal = Disposal::Purge,
                    other => config.problems.push(format!(
                        "line {line}: disposal must be trash or purge, not `{other}`"
                    )),
                },
                ("confirm_twice", Value::Bool(flag)) => config.confirm_twice = flag,
                ("min_age_days", Value::Text(days)) => match days.parse() {
                    Ok(days) => config.min_age_days = days,
                    Err(_) => config.problems.push(format!(
                        "line {line}: min_age_days must be a number, not `{days}`"
                    )),
                },
                (other, _) => config.problems.push(format!(
                    "line {line}: unknown or mistyped setting `{other}`"
                )),
            }
        }

        config
    }
}

/// A parsed right-hand side.
enum Value {
    Text(String),
    Bool(bool),
    List(Vec<String>),
}

/// Splits the file into `(key, value, line number)` triples.
///
/// Arrays may span several lines; everything else is a single line.
fn entries(text: &str) -> Vec<(String, Value, usize)> {
    let mut found = Vec::new();
    let mut lines = text.lines().enumerate();

    while let Some((index, raw)) = lines.next() {
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }

        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        let mut rest = rest.trim().to_string();

        if rest.starts_with('[') {
            // Keep pulling lines until the array closes.
            while !rest.contains(']') {
                let Some((_, more)) = lines.next() else { break };
                rest.push(' ');
                rest.push_str(strip_comment(more));
            }
            let inner = rest
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or_default()
                .to_string();
            let items = inner
                .split(',')
                .map(|item| unquote(item.trim()))
                .filter(|item| !item.is_empty())
                .collect();
            found.push((key, Value::List(items), index + 1));
            continue;
        }

        let value = match rest.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            other => Value::Text(unquote(other)),
        };
        found.push((key, value, index + 1));
    }

    found
}

fn strip_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or_default().trim()
}

fn unquote(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string()
}

/// Paths the tool must leave alone.
#[derive(Debug, Clone, Default)]
pub struct Excludes {
    patterns: Vec<String>,
}

impl Excludes {
    pub fn new(patterns: Vec<String>) -> Self {
        Self { patterns }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Adds patterns given on the command line.
    pub fn extend(&mut self, patterns: &[String]) {
        self.patterns.extend(patterns.iter().cloned());
    }

    /// Whether a path is protected — either directly, or by an ancestor.
    pub fn blocks(&self, path: &Path) -> bool {
        if self.patterns.is_empty() {
            return false;
        }

        let text = path.to_string_lossy().to_string();
        let short = crate::format::tilde(path);

        for pattern in &self.patterns {
            let expanded = expand_tilde(pattern);
            if matches_path(&expanded, &text) || matches_path(pattern, &short) {
                return true;
            }
            // A protected directory protects everything under it.
            for ancestor in path.ancestors().skip(1) {
                let ancestor_text = ancestor.to_string_lossy().to_string();
                if matches_path(&expanded, &ancestor_text)
                    || matches_path(pattern, &crate::format::tilde(ancestor))
                {
                    return true;
                }
            }
        }

        false
    }
}

fn expand_tilde(pattern: &str) -> String {
    match pattern.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", fsx::home().display()),
        None => pattern.to_string(),
    }
}

/// Glob match over a path: `*` inside a segment, `**` across segments, `?` one
/// character. A pattern with no slash matches any single path component.
pub fn matches_path(pattern: &str, path: &str) -> bool {
    if !pattern.contains('/') {
        return path.split('/').any(|part| matches_segment(pattern, part));
    }

    let pattern_parts: Vec<&str> = pattern.split('/').filter(|p| !p.is_empty()).collect();
    let path_parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    matches_parts(&pattern_parts, &path_parts)
}

fn matches_parts(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.first() {
        None => path.is_empty(),
        Some(&"**") => {
            // `**` swallows zero or more segments.
            (0..=path.len()).any(|skip| matches_parts(&pattern[1..], &path[skip..]))
        }
        Some(head) => match path.first() {
            Some(part) if matches_segment(head, part) => matches_parts(&pattern[1..], &path[1..]),
            _ => false,
        },
    }
}

/// Wildcard match inside a single path component.
///
/// Case-insensitive, because the disk this runs on is: macOS formats APFS
/// without case sensitivity by default, so `Downloads` and `downloads` name
/// the same directory and `-x '*.dmg'` has to cover `Installer.DMG`. Erring
/// wide is the harmless direction here — every use of this is an exclusion,
/// and a pattern that matches too much protects a file too many.
fn matches_segment(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();

    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut resume) = (None, 0usize);

    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            resume = t;
            p += 1;
        } else if let Some(mark) = star {
            // Backtrack: let the last `*` eat one more character.
            p = mark + 1;
            resume += 1;
            t = resume;
        } else {
            return false;
        }
    }

    pattern[p..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_wildcards() {
        assert!(matches_segment("*.log", "system.log"));
        assert!(matches_segment("node_modules", "node_modules"));
        assert!(!matches_segment("*.log", "system.txt"));
        assert!(matches_segment("com.*.helper", "com.acme.helper"));
        assert!(matches_segment("?oo", "foo"));
    }

    #[test]
    fn double_star_crosses_directories() {
        assert!(matches_path(
            "/Users/a/**/target",
            "/Users/a/dev/app/target"
        ));
        assert!(matches_path("~/Documents/**", "~/Documents/archives/2019"));
        assert!(!matches_path("~/Documents/**", "~/Downloads/x"));
    }

    #[test]
    fn bare_name_matches_any_component() {
        assert!(matches_path(".venv", "/Users/a/dev/app/.venv"));
        assert!(!matches_path(".venv", "/Users/a/dev/app/venv"));
    }

    #[test]
    fn ancestors_protect_their_content() {
        let excludes = Excludes::new(vec!["/keep/**".to_string()]);
        assert!(excludes.blocks(Path::new("/keep/a/b/c.txt")));
        assert!(!excludes.blocks(Path::new("/other/a")));
    }

    #[test]
    fn an_exclusion_ignores_case_like_the_disk_does() {
        // APFS is formatted without case sensitivity by default, so these
        // name one file. A pattern that missed on case was a protection the
        // user wrote, was shown back in `--dry-run`, and did not get.
        assert!(matches_segment("*.dmg", "Installer.DMG"));
        assert!(matches_segment("*.DMG", "installer.dmg"));

        let excludes = Excludes::new(vec!["~/Downloads/keep/**".to_string()]);
        assert!(excludes.blocks(Path::new("~/downloads/KEEP/invoice.pdf")));
        // Wide, not blind: a different directory is still not protected.
        assert!(!excludes.blocks(Path::new("~/Downloads/other/invoice.pdf")));
    }

    #[test]
    fn a_protected_directory_covers_what_is_under_it() {
        // Named without a wildcard, an ancestor still protects its contents:
        // the point of `-x ~/Projects` is not to have to write the glob.
        let excludes = Excludes::new(vec!["/keep".to_string()]);
        assert!(excludes.blocks(Path::new("/keep")));
        assert!(excludes.blocks(Path::new("/keep/deep/nested/file.txt")));
        // A sibling whose name merely starts the same is not swept in.
        assert!(!excludes.blocks(Path::new("/keeper/file.txt")));
    }

    #[test]
    fn no_patterns_protects_nothing() {
        // The early return has to mean "nothing was asked for", not
        // "everything matches".
        let excludes = Excludes::new(Vec::new());
        assert!(!excludes.blocks(Path::new("/anything/at/all")));
    }

    #[test]
    fn reads_the_documented_subset() {
        let config = Config::parse(
            r#"
            # a comment
            disposal = "purge"
            confirm_twice = false
            exclude = [
              "~/Documents/archives/**",   # keep these
              ".venv",
            ]
            "#,
        );
        assert_eq!(config.disposal, Disposal::Purge);
        assert!(!config.confirm_twice);
        assert_eq!(config.exclude.patterns().len(), 2);
        assert!(config.problems.is_empty());
    }

    #[test]
    fn reports_what_it_cannot_read() {
        let config = Config::parse("disposal = \"shred\"\nnonsense = 3\n");
        assert_eq!(config.problems.len(), 2);
        // An unreadable setting never silently changes behaviour.
        assert_eq!(config.disposal, Disposal::Trash);
    }
}
