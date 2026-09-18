//! Leftovers of applications that are no longer installed.
//!
//! Dragging an application to the trash takes the bundle and leaves
//! everything else: its support directory, its container, its preferences,
//! its saved state. This module finds those, by comparing the bundle
//! identifiers scattered through `~/Library` with the ones still installed.
//!
//! The matching is deliberately conservative. A directory is only ever
//! reported when its name *is* a bundle identifier, so hand-named directories
//! (`Application Support/Sublime Text`) are never attributed to anything, and
//! an identifier related to an installed one — a helper, a framework, an
//! extension — is treated as installed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::format;
use crate::sys::{
    cmd,
    fsx::{self, Move, Policy},
};

/// Bundle kinds that carry an identifier and count as "installed".
const BUNDLE_EXTENSIONS: [&str; 2] = ["app", "prefPane"];

/// Where a bundle can legitimately live, and how deep to look.
///
/// Application Support gets a deeper sweep because that is where updaters and
/// helper applications live — Google's updater, Claude's CLI — and their
/// identifiers turn up in the very directories scanned for leftovers.
const APP_ROOTS: [(&str, usize); 5] = [
    ("/Applications", 2),
    ("/System/Applications", 2),
    ("/System/Library/CoreServices", 2),
    ("/Library/PreferencePanes", 1),
    ("/Library/Application Support", 4),
];

/// One piece of a leftover.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// What it is, for the user: `preferences`, `container`…
    pub kind: &'static str,
    pub path: PathBuf,
    pub bytes: u64,
}

/// Everything left behind by one uninstalled application.
#[derive(Debug, Clone, Serialize)]
pub struct Leftover {
    pub bundle_id: String,
    pub bytes: u64,
    pub items: Vec<Item>,
}

/// The bundle identifiers of every installed application.
///
/// Both `/Applications/Foo.app` and the nested `/Applications/Utilities/Foo.app`
/// or `/Applications/Setapp/Foo.app` count, which is what keeps Setapp and the
/// Apple utilities from looking uninstalled.
pub fn installed(progress: &mut impl FnMut(&str)) -> Vec<String> {
    let mut roots: Vec<(PathBuf, usize)> = APP_ROOTS
        .iter()
        .map(|(path, depth)| (PathBuf::from(path), *depth))
        .collect();
    roots.push((fsx::home_join("Applications"), 2));
    roots.push((fsx::home_join("Library/PreferencePanes"), 1));
    roots.push((fsx::home_join("Library/Application Support"), 4));

    let mut bundles = Vec::new();
    for (root, depth) in roots.iter().filter(|(root, _)| root.is_dir()) {
        collect_bundles(root, *depth, &mut bundles);
    }

    let mut ids = Vec::new();
    for bundle in bundles {
        progress(&bundle.file_stem().unwrap_or_default().to_string_lossy());
        if let Some(id) = bundle_id(&bundle) {
            ids.push(id);
        }
    }

    ids.sort();
    ids.dedup();
    ids
}

fn collect_bundles(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_bundle = path
            .extension()
            .is_some_and(|ext| BUNDLE_EXTENSIONS.iter().any(|kind| ext == *kind));

        if is_bundle {
            // No descent: the helpers inside belong to this identifier anyway.
            found.push(path);
        } else if depth > 0 && path.is_dir() {
            collect_bundles(&path, depth - 1, found);
        }
    }
}

/// Reads `CFBundleIdentifier` out of a bundle, binary plist or not.
pub fn bundle_id(bundle: &Path) -> Option<String> {
    let plist = bundle.join("Contents/Info.plist");
    if !plist.is_file() {
        return None;
    }
    let output = cmd::run(
        "plutil",
        &[
            "-extract",
            "CFBundleIdentifier",
            "raw",
            "-o",
            "-",
            &plist.to_string_lossy(),
        ],
    )
    .ok()?;

    let id = output.stdout.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// The places an application leaves things behind, and what to call them.
fn haunts() -> Vec<(&'static str, PathBuf, Option<&'static str>)> {
    vec![
        (
            "support",
            fsx::home_join("Library/Application Support"),
            None,
        ),
        ("container", fsx::home_join("Library/Containers"), None),
        (
            "group container",
            fsx::home_join("Library/Group Containers"),
            None,
        ),
        ("cache", fsx::home_join("Library/Caches"), None),
        ("logs", fsx::home_join("Library/Logs"), None),
        ("web storage", fsx::home_join("Library/HTTPStorages"), None),
        ("web data", fsx::home_join("Library/WebKit"), None),
        (
            "preferences",
            fsx::home_join("Library/Preferences"),
            Some("plist"),
        ),
        (
            "saved state",
            fsx::home_join("Library/Saved Application State"),
            Some("savedState"),
        ),
        (
            "cookies",
            fsx::home_join("Library/Cookies"),
            Some("binarycookies"),
        ),
        (
            "startup agent",
            fsx::home_join("Library/LaunchAgents"),
            Some("plist"),
        ),
    ]
}

/// Whether a name is a bundle identifier rather than a human-chosen name.
///
/// Two dots at least: `com.acme.app`. A one-word directory could be anything,
/// and guessing is how a cleaner eats someone's data.
fn looks_like_bundle_id(name: &str) -> bool {
    name.matches('.').count() >= 2
        && !name.starts_with('.')
        && !name.ends_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// The identifier hidden inside a container name.
///
/// Group containers wear a team identifier in front, a `group.` marker, or
/// both: `ABCDE12345.group.com.acme` is really `com.acme`.
fn canonical_id(name: &str) -> &str {
    let name = match name.split_once('.') {
        // A team identifier is ten characters and always mixes in a digit,
        // which is what keeps `cloudflare.com.x` from being mangled.
        Some((team, rest))
            if team.len() == 10
                && team.chars().all(|c| c.is_ascii_alphanumeric())
                && team.chars().any(|c| c.is_ascii_digit()) =>
        {
            rest
        }
        _ => name,
    };

    name.strip_prefix("group.").unwrap_or(name)
}

/// Whether an identifier belongs to something still installed.
///
/// A helper (`com.acme.app.helper`) counts as installed when its application
/// is, and so does the other way round.
fn is_installed(id: &str, installed: &[String]) -> bool {
    if id.starts_with("com.apple.") || id == "com.apple" {
        return true;
    }
    installed.iter().any(|known| {
        known == id || id.starts_with(&format!("{known}.")) || known.starts_with(&format!("{id}."))
    })
}

/// Finds everything left behind by applications that are gone.
pub fn find(installed_ids: &[String], progress: &mut impl FnMut(&str)) -> Vec<Leftover> {
    let mut grouped: BTreeMap<String, Vec<Item>> = BTreeMap::new();
    let mut sizer = fsx::Sizer::new();

    for (kind, dir, extension) in haunts() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // Only the expected shape: a directory, or a file with the right
            // extension stripped off before matching.
            let stem = match extension {
                None => {
                    if !path.is_dir() {
                        continue;
                    }
                    name.clone()
                }
                Some(wanted) => match name.strip_suffix(&format!(".{wanted}")) {
                    Some(stem) => stem.to_string(),
                    None => continue,
                },
            };

            let candidate = canonical_id(&stem);
            if !looks_like_bundle_id(candidate) || is_installed(candidate, installed_ids) {
                continue;
            }

            progress(candidate);
            let bytes = sizer.size_of(&path);
            grouped
                .entry(candidate.to_string())
                .or_default()
                .push(Item { kind, path, bytes });
        }
    }

    let mut leftovers: Vec<Leftover> = grouped
        .into_iter()
        .map(|(bundle_id, items)| Leftover {
            bundle_id,
            bytes: items.iter().map(|item| item.bytes).sum(),
            items,
        })
        .collect();

    leftovers.sort_by_key(|leftover| std::cmp::Reverse(leftover.bytes));
    leftovers
}

/// Everything one bundle identifier left behind, whether or not the
/// application is still installed.
///
/// Used by `uninstall`, where the identifier is known and the question is not
/// "is this an orphan?" but "what else belongs to it?".
pub fn leftovers_of(bundle_id: &str, exclude_bundle: &Path) -> Leftover {
    let mut items = Vec::new();
    let mut sizer = fsx::Sizer::new();

    for (kind, dir, extension) in haunts() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path == exclude_bundle {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();

            let stem = match extension {
                None => name.clone(),
                Some(wanted) => match name.strip_suffix(&format!(".{wanted}")) {
                    Some(stem) => stem.to_string(),
                    None => continue,
                },
            };

            let candidate = canonical_id(&stem);
            // The identifier itself, and anything belonging to it.
            if candidate != bundle_id && !candidate.starts_with(&format!("{bundle_id}.")) {
                continue;
            }

            let bytes = sizer.size_of(&path);
            items.push(Item { kind, path, bytes });
        }
    }

    Leftover {
        bundle_id: bundle_id.to_string(),
        bytes: items.iter().map(|item| item.bytes).sum(),
        items,
    }
}

/// What removing a leftover did.
#[derive(Debug, Clone, Serialize)]
pub struct Removed {
    pub bundle_id: String,
    pub freed: u64,
    pub trashed: u64,
    pub items: usize,
    pub excluded: usize,
    pub errors: Vec<String>,
    #[serde(skip)]
    pub moves: Vec<Move>,
}

/// Removes one application's leftovers.
pub fn remove(leftover: &Leftover, policy: &Policy) -> Removed {
    let mut removal = fsx::Removal::default();

    for item in &leftover.items {
        match fsx::remove(&item.path, policy) {
            Ok(result) => removal.merge(result),
            Err(err) => removal.errors.push(err),
        }
    }

    Removed {
        bundle_id: leftover.bundle_id.clone(),
        freed: removal.freed,
        trashed: removal.trashed,
        items: removal.removed,
        excluded: removal.excluded,
        errors: removal.errors,
        moves: removal.moves,
    }
}

/// Sum of the sizes of a list of leftovers.
pub fn total(leftovers: &[Leftover]) -> u64 {
    leftovers.iter().map(|leftover| leftover.bytes).sum()
}

/// A short, readable description of what a leftover is made of.
pub fn kinds(leftover: &Leftover) -> String {
    let mut kinds: Vec<&str> = leftover.items.iter().map(|item| item.kind).collect();
    kinds.dedup();
    kinds.join(", ")
}

/// The path of a leftover item, shortened for display.
pub fn short(item: &Item) -> String {
    format::tilde(&item.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_reverse_dns_names_are_attributed() {
        assert!(looks_like_bundle_id("com.acme.app"));
        assert!(looks_like_bundle_id("org.mozilla.firefox.helper"));
        // A directory someone named by hand is never claimed.
        assert!(!looks_like_bundle_id("Sublime Text"));
        assert!(!looks_like_bundle_id("Google"));
        assert!(!looks_like_bundle_id("com.acme"));
    }

    #[test]
    fn group_and_team_prefixes_are_stripped() {
        assert_eq!(canonical_id("ABCDE12345.com.acme.app"), "com.acme.app");
        assert_eq!(canonical_id("group.com.docker"), "com.docker");
        assert_eq!(canonical_id("ABCDE12345.group.com.acme"), "com.acme");
        assert_eq!(canonical_id("com.acme.app"), "com.acme.app");
        // A ten-letter first segment with no digit is a real identifier.
        assert_eq!(canonical_id("cloudflare.com.x"), "cloudflare.com.x");
    }

    #[test]
    fn apple_is_never_an_orphan() {
        assert!(is_installed("com.apple.Safari.anything", &[]));
    }

    #[test]
    fn helpers_belong_to_their_application() {
        let installed = vec!["com.acme.app".to_string()];
        assert!(is_installed("com.acme.app.helper", &installed));
        assert!(is_installed("com.acme.app", &installed));
        // A different vendor product is still an orphan.
        assert!(!is_installed("com.other.tool", &installed));
    }

    #[test]
    fn an_application_bundle_covers_its_own_framework_ids() {
        // The bundle is `com.acme.app.framework`, the app `com.acme.app`.
        let installed = vec!["com.acme.app.framework".to_string()];
        assert!(is_installed("com.acme.app", &installed));
    }
}
