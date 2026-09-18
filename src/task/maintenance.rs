//! Opérations système : DNS, Spotlight, mémoire, instantanés, mises à jour.

use super::{Ctx, Outcome, Status};
use crate::sys::cmd;

/// Vide le cache DNS du système (nécessite root).
pub fn flush_dns(ctx: &Ctx) -> Outcome {
    let action = "Cache DNS";

    if !cmd::is_root() {
        return Outcome::skipped(action, "nécessite sudo — relancez la commande avec sudo");
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
            return Outcome::failed(action, format!("{program} : {err}"));
        }
        outcome.push(format!("{program} : ok"));
    }
    outcome
}

/// Réinitialise l'index Spotlight du volume racine (nécessite root).
pub fn reindex_spotlight(ctx: &Ctx) -> Outcome {
    let action = "Index Spotlight";

    if !cmd::is_root() {
        return Outcome::skipped(action, "nécessite sudo — relancez la commande avec sudo");
    }
    if ctx.dry_run {
        return Outcome::new(action, Status::Simulated).with("mdutil -E /");
    }

    match cmd::run("mdutil", &["-E", "/"]) {
        Ok(output) => Outcome::ok(action, false).with(output.text().to_string()),
        Err(err) => Outcome::failed(action, err),
    }
}

/// Libère la mémoire inactive (nécessite root).
pub fn purge_memory(ctx: &Ctx) -> Outcome {
    let action = "Mémoire inactive";

    if !cmd::is_root() {
        return Outcome::skipped(action, "nécessite sudo — relancez la commande avec sudo");
    }
    if ctx.dry_run {
        return Outcome::new(action, Status::Simulated).with("purge");
    }

    match cmd::run("purge", &[] as &[&str]) {
        Ok(_) => Outcome::ok(action, false),
        Err(err) => Outcome::failed(action, err),
    }
}

/// Supprime les instantanés Time Machine locaux pour libérer l'espace purgeable.
pub fn thin_snapshots(ctx: &Ctx) -> Outcome {
    let action = "Instantanés Time Machine locaux";

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

/// Liste les mises à jour macOS disponibles (lecture seule).
pub fn check_updates() -> Outcome {
    let action = "Mises à jour macOS";

    // `softwareupdate -l` écrit son rapport sur stderr et sort parfois en erreur.
    let Ok(output) = cmd::run_raw("softwareupdate", &["-l"]) else {
        return Outcome::failed(action, "softwareupdate est introuvable");
    };

    let report = format!("{}\n{}", output.stdout, output.stderr);
    if report.contains("No new software available") {
        return Outcome::ok(action, false).with("système à jour");
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
        outcome.push("aucune information renvoyée par softwareupdate");
    }
    outcome
}
