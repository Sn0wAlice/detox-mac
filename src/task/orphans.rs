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
    par,
};

/// Bundle kinds that carry an identifier and count as "installed".
const BUNDLE_EXTENSIONS: [&str; 2] = ["app", "prefPane"];

/// Where an application keeps the extensions and helpers it ships with.
///
/// These carry identifiers of their own, and not ones derived from their
/// parent: `ImageOptim.app` holds `net.pornel.ImageOptimizeExtension`, which
/// is neither `net.pornel.ImageOptim` nor a child of it in the `{id}.` sense.
/// Skip them and their containers read as leftovers of installed software.
const NESTED_BUNDLE_DIRS: [&str; 6] = [
    "Contents/PlugIns",
    "Contents/Extensions",
    "Contents/XPCServices",
    "Contents/Applications",
    "Contents/Library/LoginItems",
    "Contents/Library/QuickLook",
];

/// Identifier namespaces belonging to macOS itself.
///
/// `com.apple.` is the obvious one. Shortcuts still uses the `is.workflow.`
/// namespace it carried before Apple bought it, and nothing in the name says
/// so — its two group containers looked like a third party's leftovers.
const APPLE_NAMESPACES: [&str; 2] = ["com.apple.", "is.workflow."];

/// Whether an identifier belongs to macOS, and so is never a leftover.
pub fn is_apple(id: &str) -> bool {
    APPLE_NAMESPACES
        .iter()
        .any(|namespace| id.starts_with(namespace) || id == namespace.trim_end_matches('.'))
}

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

    // Progress is reported over the applications, which is what a reader
    // recognises; their extensions are counted with them.
    let mut all = bundles.clone();
    for bundle in &bundles {
        progress(&bundle.file_stem().unwrap_or_default().to_string_lossy());
        all.extend(nested_bundles(bundle));
    }

    // One `plutil` per bundle, and the nested ones triple the count — enough
    // to be worth spreading over the cores.
    let mut ids: Vec<String> = par::map(&all, |bundle| bundle_id(bundle), |_, _| {})
        .into_iter()
        .flatten()
        .collect();

    ids.sort();
    ids.dedup();
    ids
}

/// The bundles an application carries inside itself.
///
/// Only the handful of directories Apple reserves for them, one level deep:
/// a full walk of every application is minutes of work, and an extension
/// buried deeper than this does not exist.
fn nested_bundles(bundle: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();

    for dir in NESTED_BUNDLE_DIRS {
        let Ok(entries) = std::fs::read_dir(bundle.join(dir)) else {
            continue;
        };
        // Anything with an `Info.plist` may carry an identifier; anything
        // without one cannot, and is not worth a process to find out.
        found.extend(
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.join("Contents/Info.plist").is_file()),
        );
    }

    found
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

    // macOS spells a shared container three ways, and only one of them was
    // being unwrapped — which left `systemgroup.com.apple.…` and
    // `groups.com.apple.…` looking like identifiers no installed app owns.
    for prefix in ["systemgroup.", "groups.", "group."] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest;
        }
    }

    name
}

/// Whether an identifier belongs to something still installed.
///
/// A helper (`com.acme.app.helper`) counts as installed when its application
/// is, and so does the other way round.
fn is_installed(id: &str, installed: &[String]) -> bool {
    if is_apple(id) {
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

/// Every entry in the usual haunts that belongs to one identifier, unmeasured.
///
/// Walking a container to add up its bytes is the slow part, so it is left to
/// the caller: asking *whether* something is there, or when it was last
/// written, costs one `read_dir` per haunt.
pub fn support_entries(bundle_id: &str, exclude_bundle: &Path) -> Vec<(&'static str, PathBuf)> {
    let mut found = Vec::new();

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

            found.push((kind, path));
        }
    }

    found
}

/// Everything one bundle identifier left behind, whether or not the
/// application is still installed.
///
/// Used by `uninstall`, where the identifier is known and the question is not
/// "is this an orphan?" but "what else belongs to it?".
pub fn leftovers_of(bundle_id: &str, exclude_bundle: &Path) -> Leftover {
    let mut sizer = fsx::Sizer::new();
    let items: Vec<Item> = support_entries(bundle_id, exclude_bundle)
        .into_iter()
        .map(|(kind, path)| {
            let bytes = sizer.size_of(&path);
            Item { kind, path, bytes }
        })
        .collect();

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

    #[test]
    fn every_spelling_of_a_shared_container_is_unwrapped() {
        // All three wrap an identifier that belongs to installed software.
        // Only `group.` was being stripped, so the other two reached the
        // report as identifiers nothing owned — one of them Find My's.
        assert_eq!(canonical_id("group.com.acme.app"), "com.acme.app");
        assert_eq!(
            canonical_id("groups.com.apple.podcasts"),
            "com.apple.podcasts"
        );
        assert_eq!(
            canonical_id("systemgroup.com.apple.icloud.searchpartyd.sharedsettings"),
            "com.apple.icloud.searchpartyd.sharedsettings"
        );
        // A team identifier in front of any of them comes off first.
        assert_eq!(
            canonical_id("243LU875E5.groups.com.apple.podcasts"),
            "com.apple.podcasts"
        );
    }

    #[test]
    fn apple_group_containers_are_never_orphans() {
        // What this pins, end to end: these four were reported as removable
        // leftovers on a machine where Shortcuts, Podcasts and Find My were
        // all installed and working.
        for name in [
            "systemgroup.com.apple.icloud.searchpartyd.sharedsettings",
            "243LU875E5.groups.com.apple.podcasts",
            "group.is.workflow.my.app",
            "group.is.workflow.shortcuts",
        ] {
            assert!(
                is_installed(canonical_id(name), &[]),
                "{name} belongs to macOS and must never be offered for deletion"
            );
        }
    }

    #[test]
    fn shortcuts_keeps_the_namespace_it_was_bought_with() {
        // `is.workflow.*` is Apple — Shortcuts.app declares exactly these as
        // its app groups — and no part of the name says so.
        assert!(is_apple("is.workflow.my.app"));
        assert!(is_apple("is.workflow.shortcuts"));
        assert!(is_apple("com.apple.Safari"));
        // The namespace, not a lookalike vendor sitting next to it.
        assert!(!is_apple("is.workflowy.app"));
        assert!(!is_apple("com.appleseed.tool"));
        assert!(!is_apple("net.pornel.ImageOptim"));
    }

    #[test]
    fn a_name_that_is_only_a_prefix_is_not_a_child() {
        // The gap that hid ImageOptim's extension: the identifier starts
        // with the application's, but the next character is not a dot, so no
        // relation rule covers it. It has to come from the bundle itself.
        let installed = vec!["net.pornel.ImageOptim".to_string()];
        assert!(!is_installed(
            "net.pornel.ImageOptimizeExtension",
            &installed
        ));

        // Which it does, once the extension inside the bundle is read.
        let installed = vec![
            "net.pornel.ImageOptim".to_string(),
            "net.pornel.ImageOptimizeExtension".to_string(),
        ];
        assert!(is_installed(
            "net.pornel.ImageOptimizeExtension",
            &installed
        ));
    }

    #[test]
    fn extensions_inside_an_application_are_found() {
        let root = std::env::temp_dir().join("detox-mac-test-nested-bundles");
        let _ = std::fs::remove_dir_all(&root);
        let app = root.join("Demo.app");

        // An extension, exactly where macOS puts one.
        let appex = app.join("Contents/PlugIns/Share.appex/Contents");
        std::fs::create_dir_all(&appex).unwrap();
        std::fs::write(appex.join("Info.plist"), b"stub").unwrap();

        // A resource directory in the same place that carries no identifier,
        // and so is not worth a process to interrogate.
        std::fs::create_dir_all(app.join("Contents/PlugIns/Assets.bundle")).unwrap();

        let found = nested_bundles(&app);
        assert_eq!(found.len(), 1, "found {found:?}");
        assert!(found[0].ends_with("Share.appex"));

        // Nothing nested, nothing claimed — and no panic on an application
        // that has no PlugIns directory at all.
        assert!(nested_bundles(&root.join("Bare.app")).is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }
}
