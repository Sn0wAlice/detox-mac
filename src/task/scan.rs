//! Inventaire de l'espace disque : applications et gros fichiers.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::format;
use crate::sys::fsx;

/// Un élément pesé sur le disque.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// Nom affiché (application) ou chemin abrégé (fichier).
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
}

/// Applications installées, de la plus lourde à la plus légère.
pub fn applications() -> Vec<Entry> {
    let roots = [
        PathBuf::from("/Applications"),
        fsx::home_join("Applications"),
    ];

    let mut apps = Vec::new();
    for root in roots.iter().filter(|r| r.is_dir()) {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "app") {
                apps.push(Entry {
                    name: path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    bytes: fsx::size_of(&path),
                    path,
                });
            }
        }
    }

    apps.sort_by_key(|a| std::cmp::Reverse(a.bytes));
    apps
}

/// Fichiers dépassant `min_size` sous `root`, du plus gros au plus petit.
pub fn large_files(root: &Path, min_size: u64, max_depth: usize) -> Vec<Entry> {
    let mut files = Vec::new();

    fsx::walk_files(root, max_depth, &mut |path, size| {
        if size >= min_size {
            files.push(Entry {
                name: format::tilde(path),
                path: path.to_path_buf(),
                bytes: size,
            });
        }
    });

    files.sort_by_key(|a| std::cmp::Reverse(a.bytes));
    files
}

/// Somme des tailles d'une liste d'éléments.
pub fn total(entries: &[Entry]) -> u64 {
    entries.iter().map(|e| e.bytes).sum()
}
