use std::fs;
use std::path::Path;

pub fn clean_trash() {
    let trash = format!("{}/.Trash", std::env::var("HOME").unwrap());
    let path = Path::new(&trash);
    if path.exists() {
        println!("🗑️  Emptying Trash...");
        if let Err(e) = fs::remove_dir_all(path) {
            eprintln!("⚠️  Failed to empty trash: {}", e);
        }
    }
}