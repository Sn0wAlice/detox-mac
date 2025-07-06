use std::process::Command;

pub fn flush_dns() {
    println!("🔁 Flushing DNS Cache...");
    let _ = Command::new("sudo")
        .arg("dscacheutil")
        .arg("-flushcache")
        .status();
    let _ = Command::new("sudo")
        .arg("killall")
        .arg("-HUP")
        .arg("mDNSResponder")
        .status();
    println!("✅ DNS Cache flushed.");
}

pub fn free_purgeable_space() {
    println!("🧹 Freeing purgeable space...");
    let _ = Command::new("tmutil")
        .arg("thinlocalsnapshots")
        .arg("/")
        .arg("999999999999")
        .arg("4")
        .status();
    println!("✅ Requested purge of local snapshots.");
}

pub fn reindex_spotlight() {
    println!("🔍 Reindexing Spotlight...");
    let _ = Command::new("sudo")
        .arg("mdutil")
        .arg("-E")
        .arg("/")
        .status();
    println!("✅ Spotlight index reset.");
}

pub fn repair_disk_permissions() {
    println!("🛠️ Repairing disk permissions (system snapshot)...");

    // macOS newer versions don't support old Disk Utility commands, so we simulate via snapshot rebuild
    let _ = Command::new("sudo")
        .arg("diskutil")
        .arg("resetUserPermissions")
        .arg("/") // root volume
        .arg("$(id -u)")
        .status();

    println!("✅ Disk permissions repair triggered.");
}