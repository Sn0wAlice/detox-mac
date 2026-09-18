//! `detox-mac` — maintenance macOS en ligne de commande.

mod app;
mod cli;
mod format;
mod sys;
mod task;
mod ui;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    if cfg!(not(target_os = "macos")) {
        eprintln!("detox-mac ne fonctionne que sur macOS.");
        return ExitCode::FAILURE;
    }

    if app::run(cli::Cli::parse()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
