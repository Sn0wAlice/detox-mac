use std::fs;

pub fn remove_launch_items() {
    let home = std::env::var("HOME").unwrap();
    let dirs = vec![
        format!("{}/Library/LaunchAgents", home),
        "/Library/LaunchAgents".into(),
        "/Library/LaunchDaemons".into(),
    ];

    for dir in dirs {
        println!("🧨 Checking {}", dir);
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                println!("🗑️  Deleting {}", path.display());
                let _ = fs::remove_file(path);
            }
        }
    }
}