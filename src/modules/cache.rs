use std::fs;
use std::path::Path;

pub fn clean_cache() {
    let paths = vec![
        "/Library/Caches",
        "/System/Library/Caches",
        "/private/var/folders",
    ];

    for dir in paths {
        let path = Path::new(&dir);
        if path.exists() {
            println!("🧹 Cleaning: {}", dir);
            if let Err(e) = fs::remove_dir_all(path) {
                eprintln!("⚠️  Failed to remove {}: {}", dir, e);
            }
        }
    }
}