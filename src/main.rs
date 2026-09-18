//! `detox-mac` — macOS maintenance from the command line.

mod app;
mod cli;
mod config;
mod format;
mod sys;
mod task;
mod ui;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    if cfg!(not(target_os = "macos")) {
        eprintln!("detox only runs on macOS.");
        return ExitCode::FAILURE;
    }

    if app::run(cli::Cli::parse()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
