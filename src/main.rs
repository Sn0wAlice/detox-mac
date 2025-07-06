mod cli;
mod modules;

use clap::Parser;
use cli::{Cli, Commands};
use modules::{cache, launch, trash, system, login};

fn main() {
    let args = Cli::parse();

    match args.command {
        Commands::CleanCache => cache::clean_cache(),
        Commands::CleanTrash => trash::clean_trash(),
        Commands::RemoveLaunchItems => launch::remove_launch_items(),
        Commands::CleanAll { skip_confirmation } => {
            if skip_confirmation || confirm_clean_all() {
                cache::clean_cache();
                trash::clean_trash();
                launch::remove_launch_items();
                println!("✅ All cleaning tasks completed successfully!");
            } else {
                println!("❌ Cleaning aborted by user.");
            }
        }
        Commands::FlushDns => system::flush_dns(),
        Commands::FreeSpace => system::free_purgeable_space(),
        Commands::ReindexSpotlight => system::reindex_spotlight(),
        Commands::RepairPermissions => system::repair_disk_permissions(),
        Commands::ListLoginItems => login::list_login_items(),
        Commands::RemoveLoginItem { name } => login::remove_login_item(&name),
    }
}

fn confirm_clean_all() -> bool {
    use std::io::{self, Write};

    print!("Are you sure you want to clean all caches, trash, and launch items? (yes/no): ");
    io::stdout().flush().unwrap(); // Ensure prompt is displayed before reading input

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim().to_lowercase();

    input == "yes" || input == "y"
}
