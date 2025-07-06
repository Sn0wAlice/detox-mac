use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "detox-mac")]
#[command(about = "A fast and modular Mac cleaner in Rust 🦀")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Clean all caches, trash, and launch items
    CleanAll {
        /// Skip confirmation prompt
        #[arg(short, long)]
        skip_confirmation: bool,
    },
    /// Clean user and system caches
    CleanCache,
    /// Empty trash bins
    CleanTrash,
    /// Remove LaunchAgents and Login Items
    RemoveLaunchItems,
    /// Flush macOS DNS cache
    FlushDns,
    /// Free up purgeable system space
    FreeSpace,
    /// Reindex Spotlight
    ReindexSpotlight,
    /// Repair Disk Permissions
    RepairPermissions,
    /// List login and background items
    ListLoginItems,
    /// Remove a login or background item by name
    RemoveLoginItem {
        /// Name of the `.plist` file to remove (partial match allowed)
        name: String,
    },
}
