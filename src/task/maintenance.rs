//! System operations: DNS, Spotlight, memory, snapshots, updates.

use super::{Ctx, Outcome, Status};
use crate::sys::cmd;

/// Flushes the system DNS cache (requires root).
pub fn flush_dns(ctx: &Ctx) -> Outcome {
    let action = "DNS cache";

    if !cmd::is_root() {
        return Outcome::skipped(action, "requires sudo — run the command again with sudo");
    }
    if ctx.dry_run {
        return Outcome::new(action, Status::Simulated)
            .with("dscacheutil -flushcache")
            .with("killall -HUP mDNSResponder");
    }

    let mut outcome = Outcome::ok(action, false);
    for (program, args) in [
        ("dscacheutil", vec!["-flushcache"]),
        ("killall", vec!["-HUP", "mDNSResponder"]),
    ] {
        if let Err(err) = cmd::run(program, &args) {
            return Outcome::failed(action, format!("{program}: {err}"));
        }
        outcome.push(format!("{program}: ok"));
    }
    outcome
}

/// Rebuilds the Spotlight index of the boot volume (requires root).
pub fn reindex_spotlight(ctx: &Ctx) -> Outcome {
    let action = "Spotlight index";

    if !cmd::is_root() {
        return Outcome::skipped(action, "requires sudo — run the command again with sudo");
    }
    if ctx.dry_run {
        return Outcome::new(action, Status::Simulated).with("mdutil -E /");
    }

    match cmd::run("mdutil", &["-E", "/"]) {
        Ok(output) => Outcome::ok(action, false).with(output.text().to_string()),
        Err(err) => Outcome::failed(action, err),
    }
}

/// Frees inactive memory (requires root).
pub fn purge_memory(ctx: &Ctx) -> Outcome {
    let action = "Inactive memory";

    if !cmd::is_root() {
        return Outcome::skipped(action, "requires sudo — run the command again with sudo");
    }
    if ctx.dry_run {
        return Outcome::new(action, Status::Simulated).with("purge");
    }

    match cmd::run("purge", &[] as &[&str]) {
        Ok(_) => Outcome::ok(action, false),
        Err(err) => Outcome::failed(action, err),
    }
}

/// Removes local Time Machine snapshots to free purgeable space.
pub fn thin_snapshots(ctx: &Ctx) -> Outcome {
    let action = "Local Time Machine snapshots";

    if ctx.dry_run {
        return Outcome::new(action, Status::Simulated)
            .with("tmutil thinlocalsnapshots / 999999999999 4");
    }

    match cmd::run("tmutil", &["thinlocalsnapshots", "/", "999999999999", "4"]) {
        Ok(output) => {
            let mut outcome = Outcome::ok(action, false);
            if !output.text().is_empty() {
                outcome.push(output.text().to_string());
            }
            outcome
        }
        Err(err) => Outcome::failed(action, err),
    }
}

/// Lists available macOS updates (read only).
pub fn check_updates() -> Outcome {
    let action = "macOS updates";

    // `softwareupdate -l` writes its report on stderr and sometimes exits non-zero.
    let Ok(output) = cmd::run_raw("softwareupdate", &["-l"]) else {
        return Outcome::failed(action, "softwareupdate was not found");
    };

    let report = format!("{}\n{}", output.stdout, output.stderr);
    if report.contains("No new software available") {
        return Outcome::ok(action, false).with("system is up to date");
    }

    let mut outcome = Outcome::ok(action, false);
    for line in report.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("Software Update Tool")
            || line.starts_with("Finding available software")
        {
            continue;
        }
        outcome.push(line.to_string());
    }

    if outcome.messages.is_empty() {
        outcome.push("softwareupdate returned nothing");
    }
    outcome
}
