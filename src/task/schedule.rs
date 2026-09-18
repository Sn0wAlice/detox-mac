//! Running a cleanup on its own, through `launchd`.
//!
//! The tool already reads, writes, loads and unloads startup agents for other
//! people's software; this is it doing the same for itself.
//!
//! A scheduled run answers its own confirmations, so the second gate never
//! gets a chance to protect anything. That is why only targets that cost
//! nothing to lose can be scheduled, and why `--purge` cannot be.

use std::path::PathBuf;

use serde::Serialize;

use super::clean::{Risk, Target};
use super::{Ctx, Outcome, Status};
use crate::format;
use crate::sys::{cmd, fsx};

/// Label of the agent this tool installs for itself.
pub const LABEL: &str = "com.detox-mac.cleanup";

/// How often the cleanup runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
pub enum Cadence {
    /// Every day.
    Daily,
    /// Every Sunday.
    Weekly,
    /// Remove the schedule.
    Off,
}

/// Where the agent definition lives.
pub fn plist_path() -> PathBuf {
    fsx::home_join(&format!("Library/LaunchAgents/{LABEL}.plist"))
}

/// Where a scheduled run writes what it did.
fn log_path() -> PathBuf {
    super::journal::dir()
        .parent()
        .unwrap_or(&fsx::home())
        .join("schedule.log")
}

/// Whether a target is safe to hand to an unattended run.
///
/// Anything that holds data is refused: nobody is at the keyboard to read the
/// warning, and `--yes` would wave it through.
pub fn refuse(targets: &[Target]) -> Option<String> {
    let unsafe_targets: Vec<&str> = targets
        .iter()
        .filter(|target| target.risk() == Risk::Data)
        .map(|target| target.slug())
        .collect();

    if unsafe_targets.is_empty() {
        return None;
    }
    Some(format!(
        "{} hold(s) your data — a scheduled run answers its own questions, so it cannot be trusted with that",
        unsafe_targets.join(", ")
    ))
}

/// The agent definition, as `launchd` wants it.
fn plist(program: &str, targets: &[Target], cadence: Cadence, hour: u32) -> String {
    let mut arguments = vec![program.to_string(), "clean".to_string()];
    arguments.extend(targets.iter().map(|target| target.slug().to_string()));
    arguments.push("--yes".to_string());
    arguments.push("--quiet".to_string());

    let args: String = arguments
        .iter()
        .map(|argument| format!("    <string>{}</string>\n", escape(argument)))
        .collect();

    // Sunday, or every day.
    let when = match cadence {
        Cadence::Weekly => format!(
            "    <key>Weekday</key><integer>0</integer>\n    <key>Hour</key><integer>{hour}</integer>\n    <key>Minute</key><integer>0</integer>\n"
        ),
        _ => format!(
            "    <key>Hour</key><integer>{hour}</integer>\n    <key>Minute</key><integer>0</integer>\n"
        ),
    };

    let log = escape(&log_path().to_string_lossy());

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{args}  </array>
  <key>StartCalendarInterval</key>
  <dict>
{when}  </dict>
  <key>RunAtLoad</key>
  <false/>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#
    )
}

/// XML has five characters that cannot appear raw; a path can hold all of them.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Installs, replaces or removes the schedule.
pub fn apply(cadence: Cadence, targets: &[Target], hour: u32, ctx: &Ctx) -> Outcome {
    let action = match cadence {
        Cadence::Off => "Scheduled cleanup removed".to_string(),
        Cadence::Daily => "Daily cleanup".to_string(),
        Cadence::Weekly => "Weekly cleanup".to_string(),
    };
    let path = plist_path();

    if cadence == Cadence::Off {
        if !path.exists() {
            return Outcome::skipped(action, "nothing was scheduled");
        }
        if ctx.dry_run {
            return Outcome::new(action, Status::Simulated).with(format::tilde(&path));
        }

        let _ = cmd::run("launchctl", &["unload", "-w", &path.to_string_lossy()]);
        return match std::fs::remove_file(&path) {
            Ok(()) => Outcome::ok(action, false).with(format::tilde(&path)),
            Err(err) => Outcome::failed(action, format!("{}: {err}", format::tilde(&path))),
        };
    }

    if let Some(reason) = refuse(targets) {
        return Outcome::failed(action, reason);
    }
    if ctx.purge {
        return Outcome::failed(
            action,
            "--purge cannot be scheduled: an unattended run must stay undoable",
        );
    }

    let Ok(program) = std::env::current_exe() else {
        return Outcome::failed(action, "cannot find where this binary lives");
    };
    let body = plist(&program.to_string_lossy(), targets, cadence, hour);

    let slugs: Vec<&str> = targets.iter().map(|target| target.slug()).collect();
    let summary = format!(
        "{} at {hour:02}:00 — clean {}",
        match cadence {
            Cadence::Daily => "every day",
            _ => "every Sunday",
        },
        slugs.join(", ")
    );

    if ctx.dry_run {
        return Outcome::new(action, Status::Simulated)
            .with(summary)
            .with(format::tilde(&path));
    }

    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            return Outcome::failed(action, format!("{}: {err}", format::tilde(parent)));
        }
    }
    // Replacing a schedule means unloading the old one first.
    let _ = cmd::run("launchctl", &["unload", "-w", &path.to_string_lossy()]);

    if let Err(err) = std::fs::write(&path, body) {
        return Outcome::failed(action, format!("{}: {err}", format::tilde(&path)));
    }

    let mut outcome = Outcome::ok(action, false).with(summary);
    match cmd::run("launchctl", &["load", "-w", &path.to_string_lossy()]) {
        Ok(_) => outcome.push(format::tilde(&path)),
        Err(err) => outcome.push(format!("written, but launchctl refused it: {err}")),
    }
    outcome.push(format!("log: {}", format::tilde(&log_path())));
    outcome
}

/// What is scheduled right now, if anything.
pub fn current() -> Option<String> {
    let text = std::fs::read_to_string(plist_path()).ok()?;

    // The targets sit between `clean` and the first option.
    let slugs: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("<string>")
                .and_then(|rest| rest.strip_suffix("</string>"))
                .map(str::to_string)
        })
        .skip_while(|value| value != "clean")
        .skip(1)
        .take_while(|value| !value.starts_with("--"))
        .collect();

    let weekly = text.contains("<key>Weekday</key>");
    Some(format!(
        "{} — clean {}",
        if weekly { "every Sunday" } else { "every day" },
        slugs.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_targets_cannot_be_scheduled() {
        // Nobody is there to answer the second question at 3am.
        assert!(refuse(&[Target::Cache]).is_none());
        assert!(refuse(&[Target::Cache, Target::Trash]).is_some());
        assert!(refuse(&[Target::IosBackups]).is_some());
    }

    #[test]
    fn the_definition_names_the_binary_and_the_targets() {
        let body = plist(
            "/usr/local/bin/detox",
            &[Target::Cache, Target::Logs],
            Cadence::Weekly,
            3,
        );
        assert!(body.contains("<string>/usr/local/bin/detox</string>"));
        assert!(body.contains("<string>cache</string>"));
        assert!(body.contains("<string>logs</string>"));
        assert!(body.contains("<string>--yes</string>"));
        assert!(body.contains("<key>Weekday</key>"));
    }

    #[test]
    fn a_daily_schedule_has_no_weekday() {
        let body = plist("/bin/detox", &[Target::Cache], Cadence::Daily, 9);
        assert!(!body.contains("<key>Weekday</key>"));
        assert!(body.contains("<integer>9</integer>"));
    }

    #[test]
    fn paths_with_xml_characters_do_not_break_the_file() {
        let body = plist("/Users/a&b/detox", &[Target::Cache], Cadence::Daily, 3);
        assert!(body.contains("/Users/a&amp;b/detox"));
        assert!(!body.contains("a&b"));
    }
}
