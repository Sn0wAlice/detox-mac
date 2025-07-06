use std::fs;
use std::path::Path;

pub fn list_login_items() {
    let home = std::env::var("HOME").unwrap();
    let launch_agents_user = format!("{}/Library/LaunchAgents", home);
    let launch_agents_sys = "/Library/LaunchAgents";
    let launch_daemons_sys = "/Library/LaunchDaemons";

    println!("📦 Listing Login & Background Items...\n");

    // User LaunchAgents
    println!("👤 User LaunchAgents:");
    list_plist_files(&launch_agents_user);

    // System LaunchAgents
    println!("\n🖥️  System LaunchAgents:");
    list_plist_files(launch_agents_sys);

    // System LaunchDaemons
    println!("\n🧩 System LaunchDaemons:");
    list_plist_files(launch_daemons_sys);

    // Optional: macOS Ventura+ items (not directly accessible via API, more advanced)
}

fn list_plist_files(dir: &str) {
    let path = Path::new(dir);
    if !path.exists() {
        println!("(Not found)");
        return;
    }

    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            println!("  🔹 {}", file_name);
        }
    }
}

/// Remove login/background item by partial filename
pub fn remove_login_item(name: &str) {
    let home = std::env::var("HOME").unwrap();
    let search_dirs = vec![
        format!("{}/Library/LaunchAgents", home),
        "/Library/LaunchAgents".to_string(),
        "/Library/LaunchDaemons".to_string(),
    ];

    let mut found = false;

    for dir in &search_dirs {
        let path = Path::new(dir);
        if !path.exists() {
            continue;
        }

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.contains(name) {
                    let full_path = entry.path();
                    println!("🗑️  Removing: {}", full_path.display());
                    if let Err(e) = fs::remove_file(&full_path) {
                        eprintln!("❌ Failed to remove: {}", e);
                    } else {
                        found = true;
                    }
                }
            }
        }
    }

    if !found {
        println!("⚠️  No matching item found for '{}'", name);
    }
}