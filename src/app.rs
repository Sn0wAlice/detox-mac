//! Couche applicative : exécute les commandes et met en forme les résultats.

use std::path::PathBuf;

use clap::CommandFactory;
use serde_json::{Value, json};

use crate::cli::{AgentCommand, Cli, Command, Selection, SysCommand, TargetArg};
use crate::format;
use crate::sys::machine::Machine;
use crate::task::agents::{self, Action, Agent, Scope};
use crate::task::clean::{self, Target};
use crate::task::maintenance;
use crate::task::scan;
use crate::task::{Ctx, Outcome, Status};
use crate::ui::Printer;

/// Exécute la commande demandée. Renvoie `false` si une opération a échoué.
pub fn run(cli: Cli) -> bool {
    let options = cli.options;
    let printer = Printer::new(options.color, options.quiet || options.json);
    let ctx = Ctx {
        dry_run: options.dry_run,
        yes: options.yes,
        json: options.json,
        printer,
    };

    if ctx.dry_run && !ctx.json {
        printer.dry_run_banner();
    }

    match cli.command {
        Command::Info => info(&ctx),
        Command::Scan { targets } => scan_targets(&ctx, resolve(&targets, &Target::QUICK)),
        Command::Clean { targets } => clean_targets(&ctx, resolve(&targets, &Target::DEFAULT)),
        Command::Apps { top } => apps(&ctx, top),
        Command::Files {
            min,
            top,
            path,
            depth,
        } => files(&ctx, min, top, path, depth),
        Command::Agents { command } => match command {
            AgentCommand::List { third_party, scope } => list_agents(&ctx, third_party, scope),
            AgentCommand::Disable { selection } => act_on_agents(&ctx, Action::Disable, selection),
            AgentCommand::Enable { selection } => act_on_agents(&ctx, Action::Enable, selection),
            AgentCommand::Remove { selection } => act_on_agents(&ctx, Action::Remove, selection),
        },
        Command::Sys { command } => sys(&ctx, command),
        Command::Completions { shell } => {
            let mut command = Cli::command();
            let name = command.get_name().to_string();
            clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
            true
        }
    }
}

/// Cibles demandées, ou la sélection par défaut si l'utilisateur n'en donne pas.
fn resolve(targets: &[TargetArg], fallback: &[Target]) -> Vec<Target> {
    if targets.is_empty() {
        fallback.to_vec()
    } else {
        TargetArg::expand(targets)
    }
}

fn emit(value: Value) {
    match serde_json::to_string_pretty(&value) {
        Ok(text) => println!("{text}"),
        Err(err) => eprintln!("erreur JSON : {err}"),
    }
}

// ── info ────────────────────────────────────────────────────────────────────

fn info(ctx: &Ctx) -> bool {
    let machine = Machine::collect();
    let measures: Vec<clean::Measure> = Target::QUICK.iter().map(|&t| clean::measure(t)).collect();
    let reclaimable: u64 = measures.iter().map(|m| m.bytes).sum();
    let all_agents = agents::collect();
    let (apple, third_party) = agents::summary(&all_agents);

    if ctx.json {
        emit(json!({
            "machine": machine,
            "reclaimable": { "total": reclaimable, "targets": measures },
            "agents": { "apple": apple, "third_party": third_party },
        }));
        return true;
    }

    let p = &ctx.printer;
    p.heading("Machine");
    if !machine.host.is_empty() {
        p.field("Nom", &machine.host);
    }
    p.field("macOS", format!("{} ({})", machine.os, machine.build));
    p.field("CPU", format!("{} — {} cœurs", machine.cpu, machine.cores));
    p.field(
        "Mémoire",
        format!(
            "{} au total — {} utilisée, {} libre, {} inactive",
            format::size(machine.memory.total),
            format::size(machine.memory.used),
            format::size(machine.memory.free),
            format::size(machine.memory.inactive),
        ),
    );
    if let Some(disk) = &machine.disk {
        p.field(
            "Disque",
            format!(
                "{} libre sur {} ({} % utilisé)",
                format::size(disk.free),
                format::size(disk.total),
                disk.used_percent
            ),
        );
    }
    if !machine.uptime.is_empty() {
        p.field("Allumé depuis", &machine.uptime);
    }

    p.heading("Espace récupérable");
    for measure in &measures {
        render_measure(p, measure);
    }
    p.info(p.bold(&format!(
        "  {:<28} {:>10}",
        "Total",
        format::size(reclaimable)
    )));

    p.heading("Démarrage");
    p.field("Agents", format!("{third_party} tiers, {apple} Apple"));
    p.info("");
    p.info(p.dim("  detox-mac scan all    pour une mesure complète"));
    p.info(p.dim("  detox-mac clean all   pour tout nettoyer"));
    true
}

// ── scan ────────────────────────────────────────────────────────────────────

fn render_measure(p: &Printer, measure: &clean::Measure) {
    match &measure.unavailable {
        Some(reason) => p.info(format!(
            "  {:<28} {}",
            measure.label,
            p.dim(&format!("— {reason}"))
        )),
        None => p.info(format!(
            "  {:<28} {:>10}  {}",
            measure.label,
            format::size(measure.bytes),
            p.dim(&format!("{} élément(s)", measure.items))
        )),
    }
}

fn scan_targets(ctx: &Ctx, targets: Vec<Target>) -> bool {
    let measures: Vec<clean::Measure> = targets.iter().map(|&t| clean::measure(t)).collect();
    let total: u64 = measures.iter().map(|m| m.bytes).sum();

    if ctx.json {
        emit(json!({ "total": total, "targets": measures }));
        return true;
    }

    let p = &ctx.printer;
    p.heading("Espace récupérable");
    for measure in &measures {
        render_measure(p, measure);
    }
    p.info(p.bold(&format!("  {:<28} {:>10}", "Total", format::size(total))));
    true
}

// ── clean ───────────────────────────────────────────────────────────────────

fn clean_targets(ctx: &Ctx, targets: Vec<Target>) -> bool {
    let labels: Vec<&str> = targets.iter().map(|t| t.slug()).collect();
    if !ctx.confirm(&format!("Nettoyer : {} ?", labels.join(", "))) {
        if !ctx.json {
            ctx.printer.info("Annulé.");
        }
        return false;
    }

    let results: Vec<clean::Cleaned> = targets.iter().map(|&t| clean::clean(t, ctx)).collect();
    let freed: u64 = results.iter().map(|r| r.freed).sum();
    let failed = results.iter().any(|r| r.status.is_failure());

    if ctx.json {
        emit(json!({
            "dry_run": ctx.dry_run,
            "freed": freed,
            "results": results,
        }));
        return !failed;
    }

    let p = &ctx.printer;
    p.heading(if ctx.dry_run {
        "Nettoyage (simulation)"
    } else {
        "Nettoyage"
    });

    for result in &results {
        if result.status == Status::Skipped {
            let reason = result.messages.first().cloned().unwrap_or_default();
            p.skipped(format!(
                "{} {}",
                result.label,
                p.dim(&format!("— {reason}"))
            ));
            continue;
        }

        let details = if result.removed > 0 {
            p.dim(&format!("{} élément(s)", result.removed))
        } else {
            String::new()
        };
        let headline = format!(
            "{:<28} {:>10}  {}",
            result.label,
            format::size(result.freed),
            details
        );
        let headline = headline.trim_end().to_string();

        match result.status {
            Status::Failed => p.error(headline),
            _ => p.success(headline),
        }
        for message in &result.messages {
            p.item(p.dim(message));
        }
    }

    p.info("");
    p.info(format!(
        "  {} {}",
        if ctx.dry_run {
            "Récupérable :"
        } else {
            "Libéré :"
        },
        p.accent(&format::size(freed))
    ));
    !failed
}

// ── apps & fichiers ─────────────────────────────────────────────────────────

fn take<T: Clone>(items: &[T], top: usize) -> &[T] {
    if top == 0 {
        items
    } else {
        &items[..top.min(items.len())]
    }
}

fn apps(ctx: &Ctx, top: usize) -> bool {
    let apps = scan::applications();
    let shown = take(&apps, top);

    if ctx.json {
        emit(json!({ "total": scan::total(&apps), "count": apps.len(), "apps": shown }));
        return true;
    }

    let p = &ctx.printer;
    p.heading(&format!(
        "Applications ({} — {})",
        apps.len(),
        format::size(scan::total(&apps))
    ));
    for app in shown {
        p.info(format!("  {:>10}  {}", format::size(app.bytes), app.name));
    }
    if shown.len() < apps.len() {
        p.info(p.dim(&format!("  … et {} autre(s)", apps.len() - shown.len())));
    }
    true
}

fn files(ctx: &Ctx, min: u64, top: usize, path: Option<PathBuf>, depth: usize) -> bool {
    let root = path.unwrap_or_else(crate::sys::fsx::home);
    if !root.is_dir() {
        ctx.printer
            .error(format!("{} n'est pas un dossier", root.display()));
        return false;
    }

    let files = scan::large_files(&root, min, depth);
    let shown = take(&files, top);

    if ctx.json {
        emit(json!({
            "root": root,
            "min_size": min,
            "count": files.len(),
            "total": scan::total(&files),
            "files": shown,
        }));
        return true;
    }

    let p = &ctx.printer;
    p.heading(&format!(
        "Fichiers ≥ {} dans {} ({} — {})",
        format::size(min),
        format::tilde(&root),
        files.len(),
        format::size(scan::total(&files))
    ));
    if files.is_empty() {
        p.item(p.dim("aucun fichier ne dépasse cette taille"));
        return true;
    }
    for file in shown {
        p.info(format!("  {:>10}  {}", format::size(file.bytes), file.name));
    }
    if shown.len() < files.len() {
        p.info(p.dim(&format!("  … et {} autre(s)", files.len() - shown.len())));
    }
    true
}

// ── agents ──────────────────────────────────────────────────────────────────

fn list_agents(ctx: &Ctx, third_party_only: bool, scope: Option<Scope>) -> bool {
    let all = agents::collect();
    let selected: Vec<&Agent> = all
        .iter()
        .filter(|a| !third_party_only || !a.apple)
        .filter(|a| scope.is_none_or(|s| a.scope == s))
        .collect();

    if ctx.json {
        emit(json!({ "count": selected.len(), "agents": selected }));
        return true;
    }

    let p = &ctx.printer;
    for scope in Scope::ALL {
        let group: Vec<&&Agent> = selected.iter().filter(|a| a.scope == scope).collect();
        if group.is_empty() {
            continue;
        }
        p.heading(scope.label());
        for agent in group {
            let origin = if agent.apple { "Apple" } else { "tiers" };
            let state = if agent.loaded { "actif" } else { "inactif" };
            p.info(format!(
                "  {:<52} {}",
                agent.label,
                p.dim(&format!("{origin} · {state}"))
            ));
        }
    }

    let (apple, third_party) = agents::summary(&all);
    p.info("");
    p.info(format!("  {third_party} agent(s) tiers, {apple} Apple"));
    true
}

fn act_on_agents(ctx: &Ctx, action: Action, selection: Selection) -> bool {
    let all = agents::collect();

    let mut unknown = Vec::new();
    let selected: Vec<&Agent> = if selection.all_third_party {
        all.iter()
            .filter(|a| !a.apple)
            .filter(|a| selection.scope.is_none_or(|s| a.scope == s))
            .collect()
    } else {
        let mut found = Vec::new();
        for label in &selection.labels {
            match all.iter().find(|a| &a.label == label) {
                Some(agent) => found.push(agent),
                None => unknown.push(label.clone()),
            }
        }
        found
    };

    for label in &unknown {
        ctx.printer.warn(format!("agent introuvable : {label}"));
    }

    if selected.is_empty() {
        ctx.printer.error("aucun agent correspondant.");
        return false;
    }

    let apple_count = selected.iter().filter(|a| a.apple).count();
    if apple_count > 0 {
        ctx.printer.warn(format!(
            "{apple_count} agent(s) Apple ignoré(s) : detox-mac ne touche pas aux composants système."
        ));
    }

    let verb = match action {
        Action::Disable => "Désactiver",
        Action::Enable => "Réactiver",
        Action::Remove => "Supprimer définitivement",
    };
    if !ctx.confirm(&format!("{verb} {} agent(s) ?", selected.len())) {
        if !ctx.json {
            ctx.printer.info("Annulé.");
        }
        return false;
    }

    let outcomes: Vec<Outcome> = selected
        .iter()
        .map(|agent| agents::apply(agent, action, ctx))
        .collect();
    render_outcomes(ctx, &outcomes)
}

// ── système ─────────────────────────────────────────────────────────────────

fn sys(ctx: &Ctx, command: SysCommand) -> bool {
    let needs_confirmation = !matches!(command, SysCommand::Updates);
    let question = match command {
        SysCommand::Dns => "Vider le cache DNS ?",
        SysCommand::Spotlight => "Réinitialiser l'index Spotlight ?",
        SysCommand::Memory => "Libérer la mémoire inactive ?",
        SysCommand::Snapshots => "Purger les instantanés Time Machine locaux ?",
        SysCommand::Updates => "",
    };

    if needs_confirmation && !ctx.confirm(question) {
        if !ctx.json {
            ctx.printer.info("Annulé.");
        }
        return false;
    }

    let outcome = match command {
        SysCommand::Dns => maintenance::flush_dns(ctx),
        SysCommand::Spotlight => maintenance::reindex_spotlight(ctx),
        SysCommand::Memory => maintenance::purge_memory(ctx),
        SysCommand::Snapshots => maintenance::thin_snapshots(ctx),
        SysCommand::Updates => maintenance::check_updates(),
    };

    render_outcomes(ctx, std::slice::from_ref(&outcome))
}

fn render_outcomes(ctx: &Ctx, outcomes: &[Outcome]) -> bool {
    let failed = outcomes.iter().any(|o| o.status.is_failure());

    if ctx.json {
        emit(json!({ "dry_run": ctx.dry_run, "results": outcomes }));
        return !failed;
    }

    ctx.printer.info("");
    for outcome in outcomes {
        outcome.render(&ctx.printer);
    }
    !failed
}
