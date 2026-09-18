//! Application layer: runs commands and formats their results.

use std::path::PathBuf;

use clap::CommandFactory;
use serde_json::{Value, json};

use crate::cli::{
    AgentCommand, Cli, Command, DevFilter, FileCommand, Selection, SysCommand, TargetArg,
};
use crate::config::Config;
use crate::format;
use crate::sys::fsx::{self, Disposal};
use crate::sys::machine::Machine;
use crate::task::agents::{self, Action, Agent, Scope};
use crate::task::clean::{self, Target};
use crate::task::{Ctx, Outcome, Status, dev, journal, maintenance, orphans, ram, scan};
use crate::ui::Printer;

/// Runs the requested command. Returns `false` when something failed.
pub fn run(cli: Cli) -> bool {
    let options = cli.options;
    let printer = Printer::new(options.color, options.quiet || options.json);

    let mut config = if options.no_config {
        Config::default()
    } else {
        Config::load()
    };
    // Exclusions given on the command line add to the configured ones; one
    // never replaces the other, so a `-x` can only ever protect more.
    config.exclude.extend(&options.exclude);

    // A setting that could not be read is announced, never quietly ignored.
    for problem in &config.problems {
        printer.warn(format!("{}: {problem}", format::tilde(&Config::path())));
    }

    let ctx = Ctx {
        dry_run: options.dry_run,
        yes: options.yes,
        json: options.json,
        purge: options.purge,
        config,
        printer,
    };

    if ctx.dry_run && !ctx.json {
        printer.dry_run_banner();
    }

    match cli.command {
        Command::Info => info(&ctx),
        Command::Scan { targets } => scan_targets(&ctx, resolve(&ctx, &targets, &Target::QUICK)),
        Command::Clean { targets } => {
            clean_targets(&ctx, resolve(&ctx, &targets, &Target::DEFAULT))
        }
        Command::Apps { top } => apps(&ctx, top),
        Command::Files { command } => match command {
            FileCommand::Large {
                min,
                top,
                path,
                depth,
            } => large_files(&ctx, min, top, path, depth),
            FileCommand::Dev { filter, top } => dev_residue(&ctx, &filter, top, None, false),
            FileCommand::Clean {
                filter,
                native,
                all,
            } => dev_residue(&ctx, &filter, 0, Some(native), all),
        },
        Command::Ram {
            top,
            all,
            detail,
            min,
        } => memory(&ctx, top, all, detail, min),
        Command::Inspect { target, short } => inspect(&ctx, &target.join(" "), short),
        Command::Kill {
            target,
            force,
            system,
        } => kill(&ctx, &target.join(" "), force, system),
        Command::Orphans { clean, only, top } => leftovers(&ctx, clean, &only, top),
        Command::History { top } => history(&ctx, top),
        Command::Undo { id } => undo_run(&ctx, id.as_deref()),
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

/// The requested targets, or the default selection when none were given.
///
/// The configuration file can override that default; anything it names that
/// is not a target is reported rather than skipped.
fn resolve(ctx: &Ctx, targets: &[TargetArg], fallback: &[Target]) -> Vec<Target> {
    if !targets.is_empty() {
        return TargetArg::expand(targets);
    }

    if ctx.config.default_targets.is_empty() {
        return fallback.to_vec();
    }

    let mut chosen = Vec::new();
    for slug in &ctx.config.default_targets {
        match Target::from_slug(slug) {
            Some(target) if !chosen.contains(&target) => chosen.push(target),
            Some(_) => {}
            None => ctx
                .printer
                .warn(format!("unknown target in your configuration: `{slug}`")),
        }
    }

    if chosen.is_empty() {
        fallback.to_vec()
    } else {
        chosen
    }
}

/// Shape of the JSON output. Bumped whenever a field changes meaning, so a
/// script can tell what it is reading.
const SCHEMA: u32 = 2;

fn emit(mut value: Value) {
    if let Some(object) = value.as_object_mut() {
        object.insert("schema".to_string(), json!(SCHEMA));
    }
    match serde_json::to_string_pretty(&value) {
        Ok(text) => println!("{text}"),
        Err(err) => eprintln!("JSON error: {err}"),
    }
}

/// Keeps the first `top` items, or all of them when `top` is zero.
fn take<T>(items: &[T], top: usize) -> &[T] {
    if top == 0 {
        items
    } else {
        &items[..top.min(items.len())]
    }
}

/// Measures several targets, showing a progress bar.
fn measure_targets(ctx: &Ctx, targets: &[Target]) -> Vec<clean::Measure> {
    let mut progress = ctx.printer.bar("Measuring", targets.len());
    let measures = targets
        .iter()
        .map(|&target| {
            progress.tick(target.label());
            let measure = clean::measure(target);
            progress.advance(target.label());
            measure
        })
        .collect();
    progress.finish();
    measures
}

// ── info ────────────────────────────────────────────────────────────────────

fn info(ctx: &Ctx) -> bool {
    let machine = Machine::collect();
    let measures = measure_targets(ctx, &Target::QUICK);
    let reclaimable: u64 = measures.iter().map(|measure| measure.bytes).sum();
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
        p.field("Name", &machine.host);
    }
    p.field("macOS", format!("{} ({})", machine.os, machine.build));
    p.field("CPU", format!("{} — {} cores", machine.cpu, machine.cores));
    p.field(
        "Memory",
        format!(
            "{} total — {} used, {} free, {} inactive",
            format::size(machine.memory.total),
            format::size(machine.memory.used),
            format::size(machine.memory.free),
            format::size(machine.memory.inactive),
        ),
    );
    if machine.swap.total > 0 {
        p.field(
            "Swap",
            format!(
                "{} used of {}",
                format::size(machine.swap.used),
                format::size(machine.swap.total)
            ),
        );
    }
    if let Some(disk) = &machine.disk {
        p.field(
            "Disk",
            format!(
                "{} free of {} ({}% used)",
                format::size(disk.free),
                format::size(disk.total),
                disk.used_percent
            ),
        );
    }
    if !machine.uptime.is_empty() {
        p.field("Uptime", &machine.uptime);
    }

    p.heading("Reclaimable space");
    for measure in &measures {
        render_measure(p, measure);
    }
    p.info(p.bold(&format!(
        "  {:<28} {:>10}",
        "Total",
        format::size(reclaimable)
    )));

    p.heading("Startup");
    p.field(
        "Agents",
        format!("{third_party} third-party, {apple} Apple"),
    );

    p.heading("Safety");
    p.field(
        "Deletions",
        match ctx.disposal() {
            Disposal::Trash => "moved to the trash, undoable",
            Disposal::Purge => "permanent (--purge)",
        },
    );
    if ctx.config.exclude.is_empty() {
        p.field(
            "Protected",
            ctx.printer.dim("nothing — see the config file"),
        );
    } else {
        p.field(
            "Protected",
            format!(
                "{} pattern(s): {}",
                ctx.config.exclude.patterns().len(),
                format::truncate(&ctx.config.exclude.patterns().join(", "), 48)
            ),
        );
    }
    p.field(
        "Config",
        match &ctx.config.source {
            Some(path) => format::tilde(path),
            None => format!("{} (absent)", format::tilde(&Config::path())),
        },
    );

    p.info("");
    p.info(p.dim("  detox ram         see what is eating memory"));
    p.info(p.dim("  detox files dev   find build residue in your projects"));
    p.info(p.dim("  detox scan all    measure everything"));
    p.info(p.dim("  detox clean all   clean everything"));
    true
}

// ── scan ────────────────────────────────────────────────────────────────────

fn render_measure(p: &Printer, measure: &clean::Measure) {
    if let Some(reason) = &measure.unavailable {
        p.info(format!(
            "  {:<28} {}",
            measure.label,
            p.dim(&format!("— {reason}"))
        ));
        return;
    }

    // What it costs to lose it matters as much as how big it is.
    let mut detail = format!("{} item(s) · {}", measure.items, measure.risk.label());
    if measure.denied > 0 {
        detail.push_str(&format!(" · {} unreadable", measure.denied));
    }

    p.info(format!(
        "  {:<28} {:>10}  {}",
        measure.label,
        format::size(measure.bytes),
        p.dim(&detail)
    ));
}

/// Says so, once, when macOS is hiding part of what was asked for.
fn warn_missing_access(ctx: &Ctx, targets: &[Target]) {
    if ctx.json || !targets.iter().any(|t| t.needs_full_disk_access()) {
        return;
    }
    if fsx::has_full_disk_access() {
        return;
    }
    ctx.printer.warn(format!(
        "some of these targets are unreadable without Full Disk Access — {}",
        fsx::full_disk_access_hint()
    ));
}

/// Sorts a scan so the safest and biggest wins come first.
fn by_value(measures: &mut [clean::Measure]) {
    measures.sort_by(|a, b| a.risk.cmp(&b.risk).then_with(|| b.bytes.cmp(&a.bytes)));
}

fn scan_targets(ctx: &Ctx, targets: Vec<Target>) -> bool {
    warn_missing_access(ctx, &targets);

    let mut measures = measure_targets(ctx, &targets);
    let total: u64 = measures.iter().map(|measure| measure.bytes).sum();
    let denied: usize = measures.iter().map(|measure| measure.denied).sum();
    by_value(&mut measures);

    if ctx.json {
        emit(json!({
            "total": total,
            "unreadable": denied,
            "targets": measures,
        }));
        return true;
    }

    let p = &ctx.printer;
    p.heading("Reclaimable space");
    for measure in &measures {
        render_measure(p, measure);
    }
    p.info(p.bold(&format!("  {:<28} {:>10}", "Total", format::size(total))));

    if denied > 0 {
        p.warn(format!(
            "{denied} director{} could not be read — {}",
            if denied == 1 { "y" } else { "ies" },
            fsx::full_disk_access_hint()
        ));
    }
    true
}

// ── clean ───────────────────────────────────────────────────────────────────

/// Second gate, for what the first `y` should not be enough to authorise.
///
/// Moving a cache to the trash is reversible and stops at one question.
/// Emptying the trash, or running with `--purge`, does not.
fn second_gate(ctx: &Ctx, targets: &[Target], disposal: Disposal) -> bool {
    let irreversible: Vec<&str> = targets
        .iter()
        .filter(|target| target.is_irreversible(disposal))
        .map(|target| target.slug())
        .collect();

    if !irreversible.is_empty() {
        return ctx.confirm_final(
            &format!(
                "This cannot be undone — nothing goes to the trash: {}.",
                irreversible.join(", ")
            ),
            "delete forever",
        );
    }

    let data: Vec<&str> = targets
        .iter()
        .filter(|target| target.risk() == clean::Risk::Data)
        .map(|target| target.slug())
        .collect();

    if !data.is_empty() {
        return ctx.confirm_final(
            &format!("These hold your own data, not cache: {}.", data.join(", ")),
            "delete",
        );
    }

    true
}

/// Writes what just happened to the journal, so it can be read back — and,
/// when it went to the trash, undone.
fn record_run(ctx: &Ctx, command: &str, disposal: Disposal, removal: &fsx::Removal) {
    if ctx.dry_run {
        return;
    }
    match journal::record(command, disposal, removal) {
        Ok(Some(path)) => {
            if !ctx.json {
                ctx.printer.info(
                    ctx.printer
                        .dim(&format!("  journal: {}", format::tilde(&path))),
                );
            }
        }
        Ok(None) => {}
        Err(err) => ctx
            .printer
            .warn(format!("the journal could not be written: {err}")),
    }
}

/// The closing lines of any command that removed something.
fn render_disposal(ctx: &Ctx, freed: u64, trashed: u64) {
    let p = &ctx.printer;
    p.info("");

    if freed > 0 || trashed == 0 {
        p.info(format!(
            "  {} {}",
            if ctx.dry_run {
                "Reclaimable:"
            } else {
                "Freed:"
            },
            p.accent(&format::size(freed))
        ));
    }

    if trashed > 0 {
        p.info(format!(
            "  {} {}",
            if ctx.dry_run {
                "To the trash:"
            } else {
                "Moved to trash:"
            },
            p.accent(&format::size(trashed))
        ));
        // Saying "freed" here would be a lie: the blocks are still allocated.
        p.info(p.dim("  Still on disk until the trash is emptied — detox clean trash"));
        if !ctx.dry_run {
            p.info(p.dim("  Changed your mind? detox undo"));
        }
    }
}

fn clean_targets(ctx: &Ctx, targets: Vec<Target>) -> bool {
    warn_missing_access(ctx, &targets);

    let labels: Vec<&str> = targets.iter().map(|target| target.slug()).collect();
    let disposal = ctx.disposal();

    if !ctx.confirm(&format!("Clean: {}?", labels.join(", ")))
        || !second_gate(ctx, &targets, disposal)
    {
        if !ctx.json {
            ctx.printer.info("Cancelled.");
        }
        return false;
    }

    let mut progress = ctx.printer.bar("Cleaning", targets.len());
    let results: Vec<clean::Cleaned> = targets
        .iter()
        .map(|&target| {
            progress.tick(target.label());
            let cleaned = clean::clean(target, ctx);
            progress.advance(target.label());
            cleaned
        })
        .collect();
    progress.finish();

    let freed: u64 = results.iter().map(|result| result.freed).sum();
    let trashed: u64 = results.iter().map(|result| result.trashed).sum();
    let excluded: usize = results.iter().map(|result| result.excluded).sum();
    let failed = results.iter().any(|result| result.status.is_failure());

    let mut removal = fsx::Removal {
        freed,
        trashed,
        removed: results.iter().map(|result| result.removed).sum(),
        ..Default::default()
    };
    for result in &results {
        removal.moves.extend(result.moves.iter().cloned());
    }
    record_run(
        ctx,
        &format!("clean {}", labels.join(" ")),
        disposal,
        &removal,
    );

    if ctx.json {
        emit(json!({
            "dry_run": ctx.dry_run,
            "disposal": disposal,
            "freed": freed,
            "trashed": trashed,
            "excluded": excluded,
            "results": results,
        }));
        return !failed;
    }

    let p = &ctx.printer;
    p.heading(if ctx.dry_run {
        "Cleanup (dry run)"
    } else {
        "Cleanup"
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
            p.dim(&format!("{} item(s)", result.removed))
        } else {
            String::new()
        };
        let headline = format!(
            "{:<28} {:>10}  {}",
            result.label,
            format::size(result.total()),
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

    render_disposal(ctx, freed, trashed);
    if excluded > 0 {
        p.info(p.dim(&format!(
            "  {excluded} item(s) protected by your exclusions"
        )));
    }
    !failed
}

// ── applications and large files ────────────────────────────────────────────

fn apps(ctx: &Ctx, top: usize) -> bool {
    let bundles = scan::application_bundles();
    let mut progress = ctx.printer.bar("Measuring", bundles.len());

    let mut apps: Vec<scan::Entry> = bundles
        .iter()
        .map(|bundle| {
            progress.tick(&bundle.file_stem().unwrap_or_default().to_string_lossy());
            let entry = scan::measure_application(bundle);
            progress.advance(&entry.name);
            entry
        })
        .collect();
    progress.finish();

    apps.sort_by_key(|app| std::cmp::Reverse(app.bytes));
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
        p.info(p.dim(&format!("  … and {} more", apps.len() - shown.len())));
    }
    true
}

fn large_files(ctx: &Ctx, min: u64, top: usize, path: Option<PathBuf>, depth: usize) -> bool {
    let root = path.unwrap_or_else(fsx::home);
    if !root.is_dir() {
        ctx.printer
            .error(format!("{} is not a directory", root.display()));
        return false;
    }

    let mut progress = ctx.printer.spinner("Scanning");
    let files = scan::large_files(&root, min, depth, &mut |seen, path| {
        progress.done_count(seen);
        progress.tick(&format::tilde(path));
    });
    progress.finish();

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
        "Files ≥ {} in {} ({} — {})",
        format::size(min),
        format::tilde(&root),
        files.len(),
        format::size(scan::total(&files))
    ));
    if files.is_empty() {
        p.item(p.dim("no file is that big"));
        return true;
    }
    for file in shown {
        p.info(format!("  {:>10}  {}", format::size(file.bytes), file.name));
    }
    if shown.len() < files.len() {
        p.info(p.dim(&format!("  … and {} more", files.len() - shown.len())));
    }
    true
}

// ── build residue ───────────────────────────────────────────────────────────

fn dev_residue(
    ctx: &Ctx,
    filter: &DevFilter,
    top: usize,
    remove: Option<bool>,
    take_all: bool,
) -> bool {
    let p = &ctx.printer;
    let root = filter.path.clone().unwrap_or_else(fsx::home);

    if !root.is_dir() {
        p.error(format!("{} is not a directory", root.display()));
        return false;
    }

    let unknown: Vec<&str> = filter
        .lang
        .iter()
        .map(String::as_str)
        .filter(|wanted| {
            !dev::languages()
                .iter()
                .any(|known| known.eq_ignore_ascii_case(wanted))
        })
        .collect();
    if !unknown.is_empty() {
        p.error(format!(
            "unknown language: {} — known: {}",
            unknown.join(", "),
            dev::languages().join(", ")
        ));
        return false;
    }

    let criteria = dev::Filter {
        root: root.clone(),
        max_depth: filter.depth,
        min_age_days: filter.older_than,
        languages: filter.lang.clone(),
    };

    let mut walk = p.spinner("Looking for projects");
    let mut seen = 0usize;
    let mut residue = dev::candidates(&criteria, &mut |dir| {
        seen += 1;
        walk.done_count(seen);
        walk.tick(&format::tilde(dir));
    });
    walk.finish();

    let mut sizing = p.bar("Measuring", residue.len());
    for entry in &mut residue {
        sizing.tick(&format::tilde(&entry.project));
        dev::measure(entry);
        sizing.advance(&format::tilde(&entry.project));
    }
    sizing.finish();
    residue.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));

    // Protected paths never reach the list, so they can never be picked by a
    // careless `--yes` either.
    let before = residue.len();
    residue.retain(|entry| !ctx.config.exclude.blocks(&entry.path));
    let protected = before - residue.len();
    if protected > 0 && !ctx.json {
        p.info(p.dim(&format!(
            "  {protected} director{} protected by your exclusions",
            if protected == 1 { "y" } else { "ies" }
        )));
    }

    let total = dev::total(&residue);

    let Some(native) = remove else {
        let shown = take(&residue, top);

        if ctx.json {
            emit(json!({
                "root": root,
                "older_than_days": filter.older_than,
                "count": residue.len(),
                "total": total,
                "residue": shown,
            }));
            return true;
        }

        p.heading(&format!(
            "Build residue in {} — untouched for {} day(s) ({} director{}, {})",
            format::tilde(&root),
            filter.older_than,
            residue.len(),
            if residue.len() == 1 { "y" } else { "ies" },
            format::size(total)
        ));
        if residue.is_empty() {
            p.item(p.dim("nothing to reclaim"));
            return true;
        }
        for entry in shown {
            render_residue(p, entry);
        }
        if shown.len() < residue.len() {
            p.info(p.dim(&format!(
                "  … and {} smaller director{}",
                residue.len() - shown.len(),
                if residue.len() - shown.len() == 1 {
                    "y"
                } else {
                    "ies"
                }
            )));
        }
        p.info("");
        p.info(p.dim("  detox files clean   removes them (your next build rebuilds them)"));
        return true;
    };

    if residue.is_empty() {
        if ctx.json {
            emit(json!({ "removed": [], "freed": 0 }));
        } else {
            p.info("");
            p.skipped("No build residue to remove.");
        }
        return true;
    }

    if !ctx.json {
        p.heading(&format!(
            "{} regenerable director{}, {} — untouched for {} day(s)",
            residue.len(),
            if residue.len() == 1 { "y" } else { "ies" },
            format::size(total),
            filter.older_than
        ));
        for (index, entry) in residue.iter().take(LISTED).enumerate() {
            render_numbered_residue(p, index + 1, entry);
            if native {
                if let Some(command) = dev::planned_command(entry) {
                    p.item(p.dim(&format!("via {command}")));
                }
            }
        }
        if residue.len() > LISTED {
            p.info(p.dim(&format!("  … and {} more", residue.len() - LISTED)));
        }
    }

    // One project out of the fifty is the one you go back to tomorrow.
    // Removing all or nothing was the only option before this.
    if !take_all && !keep_some(ctx, &mut residue) {
        if !ctx.json {
            p.info("Cancelled.");
        }
        return false;
    }
    if residue.is_empty() {
        if !ctx.json {
            p.info("Nothing left to remove.");
        }
        return true;
    }
    let total = dev::total(&residue);

    let disposal = ctx.disposal();
    if !ctx.confirm(&format!(
        "Remove {} director{} ({})?",
        residue.len(),
        if residue.len() == 1 { "y" } else { "ies" },
        format::size(total)
    )) {
        if !ctx.json {
            p.info("Cancelled.");
        }
        return false;
    }

    if disposal == Disposal::Purge
        && !ctx.confirm_final(
            "This cannot be undone — nothing goes to the trash.",
            "delete forever",
        )
    {
        if !ctx.json {
            p.info("Cancelled.");
        }
        return false;
    }

    let mut progress = p.bar("Removing", residue.len());
    let removed: Vec<dev::Removed> = residue
        .iter()
        .map(|entry| {
            progress.tick(&format::tilde(&entry.project));
            let outcome = dev::remove_one(entry, native, ctx);
            progress.advance(&format::tilde(&entry.project));
            outcome
        })
        .collect();
    progress.finish();

    let accounted: u64 = removed.iter().map(|entry| entry.bytes).sum();
    let trashed: u64 = removed.iter().map(|entry| entry.trashed).sum();
    let freed = accounted.saturating_sub(trashed);
    let failed = removed.iter().filter(|entry| entry.error.is_some()).count();

    let mut removal = fsx::Removal {
        freed,
        trashed,
        removed: removed.len() - failed,
        ..Default::default()
    };
    for entry in &removed {
        removal.moves.extend(entry.moves.iter().cloned());
    }
    record_run(ctx, "files clean", disposal, &removal);

    if ctx.json {
        emit(json!({
            "dry_run": ctx.dry_run,
            "disposal": disposal,
            "freed": freed,
            "trashed": trashed,
            "removed": removed,
        }));
        return failed == 0;
    }

    p.info("");
    let summary = format!(
        "{} director{} removed",
        removed.len() - failed,
        if removed.len() - failed == 1 {
            "y"
        } else {
            "ies"
        }
    );
    if ctx.dry_run {
        p.success(format!("{summary} [dry run]"));
    } else {
        p.success(summary);
    }

    for entry in &removed {
        let detail = match (&entry.command, entry.method) {
            (Some(command), dev::Method::Toolchain) => command.clone(),
            (Some(command), dev::Method::ToolchainThenRemoved) => {
                format!("{command}, then removed the leftovers")
            }
            _ => continue,
        };
        p.item(p.dim(&format!("{} — {detail}", format::tilde(&entry.path))));
    }
    for entry in removed.iter().filter(|entry| entry.error.is_some()) {
        p.error(format!(
            "{}: {}",
            format::tilde(&entry.path),
            entry.error.clone().unwrap_or_default()
        ));
    }

    // Where those bytes actually went, and how to change your mind.
    render_disposal(ctx, freed, trashed);
    failed == 0
}

/// How many directories are listed before the tail is summarised.
const LISTED: usize = 50;

fn render_numbered_residue(p: &Printer, number: usize, residue: &dev::Residue) {
    p.info(format!(
        "  {:>3}. {:>10}  {:<16} {:<40} {}",
        number,
        format::size(residue.bytes),
        residue.kind,
        format::truncate_start(&format::tilde(&residue.project), 40),
        p.dim(&format!("{} · {}d", residue.language, residue.age_days))
    ));
}

/// Offers to drop entries from the list before anything is removed.
///
/// Returns `false` when the user cancelled outright.
fn keep_some(ctx: &Ctx, residue: &mut Vec<dev::Residue>) -> bool {
    if ctx.json || ctx.yes || ctx.dry_run || residue.len() < 2 {
        return true;
    }

    let shown = residue.len().min(LISTED);
    let Some(answer) = ctx.printer.ask(&format!(
        "Numbers to keep (e.g. 1,4-6), Enter to remove all {shown}, q to cancel:"
    )) else {
        // Not a terminal: the plain confirmation below still applies.
        return true;
    };

    if answer.eq_ignore_ascii_case("q") {
        return false;
    }
    if answer.is_empty() {
        return true;
    }

    match crate::ui::parse_ranges(&answer, shown) {
        Ok(kept) => {
            let mut index = 0;
            residue.retain(|_| {
                let keep = kept.contains(&index);
                index += 1;
                !keep
            });
            ctx.printer.info(
                ctx.printer
                    .dim(&format!("  keeping {} director(ies)", kept.len())),
            );
            true
        }
        Err(err) => {
            // A misread list would delete what the user meant to protect.
            ctx.printer.error(format!("{err} — nothing was removed."));
            false
        }
    }
}

fn render_residue(p: &Printer, residue: &dev::Residue) {
    p.info(format!(
        "  {:>10}  {:<16} {:<40} {}",
        format::size(residue.bytes),
        residue.kind,
        format::truncate_start(&format::tilde(&residue.project), 40),
        p.dim(&format!("{} · {}d", residue.language, residue.age_days))
    ));
}

// ── leftovers of uninstalled applications ───────────────────────────────────

fn leftovers(ctx: &Ctx, remove: bool, only: &[String], top: usize) -> bool {
    let p = &ctx.printer;

    let mut progress = p.spinner("Reading installed applications");
    let mut seen = 0usize;
    let installed = orphans::installed(&mut |name| {
        seen += 1;
        progress.done_count(seen);
        progress.tick(name);
    });
    progress.finish();

    if installed.is_empty() {
        p.error("not one application bundle could be read — refusing to guess what is an orphan");
        return false;
    }

    let mut progress = p.spinner("Looking for leftovers");
    let mut found = 0usize;
    let mut leftovers = orphans::find(&installed, &mut |id| {
        found += 1;
        progress.done_count(found);
        progress.tick(id);
    });
    progress.finish();

    if !only.is_empty() {
        leftovers.retain(|leftover| {
            only.iter()
                .any(|wanted| wanted.eq_ignore_ascii_case(&leftover.bundle_id))
        });
    }

    // Protected paths leave the list entirely, item by item.
    for leftover in &mut leftovers {
        leftover
            .items
            .retain(|item| !ctx.config.exclude.blocks(&item.path));
        leftover.bytes = leftover.items.iter().map(|item| item.bytes).sum();
    }
    leftovers.retain(|leftover| !leftover.items.is_empty());

    let total = orphans::total(&leftovers);

    if !remove {
        let shown = take(&leftovers, top);

        if ctx.json {
            emit(json!({
                "count": leftovers.len(),
                "total": total,
                "installed": installed.len(),
                "leftovers": shown,
            }));
            return true;
        }

        p.heading(&format!(
            "Leftovers of uninstalled applications ({} — {})",
            leftovers.len(),
            format::size(total)
        ));
        if leftovers.is_empty() {
            p.item(p.dim("nothing left behind"));
            return true;
        }
        for (index, leftover) in shown.iter().enumerate() {
            render_leftover(p, index + 1, leftover);
        }
        if shown.len() < leftovers.len() {
            p.info(p.dim(&format!("  … and {} more", leftovers.len() - shown.len())));
        }
        p.info("");
        p.info(p.dim("  Only directories named after a bundle identifier are matched, so this"));
        p.info(p.dim("  list is short of what is really there — and never guesses."));
        p.info(p.dim("  detox orphans --clean   removes them"));
        return true;
    }

    if leftovers.is_empty() {
        if ctx.json {
            emit(json!({ "removed": [], "freed": 0 }));
        } else {
            p.skipped("No leftovers to remove.");
        }
        return true;
    }

    if !ctx.json {
        p.heading(&format!(
            "{} application(s) left {} behind",
            leftovers.len(),
            format::size(total)
        ));
        for (index, leftover) in leftovers.iter().take(LISTED).enumerate() {
            render_leftover(p, index + 1, leftover);
            for item in &leftover.items {
                p.item(p.dim(&format!("    {} — {}", item.kind, orphans::short(item))));
            }
        }
    }

    let disposal = ctx.disposal();
    if !ctx.confirm(&format!(
        "Remove the leftovers of {} application(s) ({})?",
        leftovers.len(),
        format::size(total)
    )) {
        if !ctx.json {
            p.info("Cancelled.");
        }
        return false;
    }

    // Attribution by name is a heuristic, so this gate is asked whichever way
    // the entries are going.
    let warning = if disposal == Disposal::Purge {
        "These are matched by name, and this cannot be undone."
    } else {
        "These are matched by name — check the list above before answering."
    };
    if !ctx.confirm_final(warning, "delete") {
        if !ctx.json {
            p.info("Cancelled.");
        }
        return false;
    }

    let policy = ctx.policy();
    let mut progress = p.bar("Removing", leftovers.len());
    let removed: Vec<orphans::Removed> = leftovers
        .iter()
        .map(|leftover| {
            progress.tick(&leftover.bundle_id);
            let outcome = orphans::remove(leftover, &policy);
            progress.advance(&leftover.bundle_id);
            outcome
        })
        .collect();
    progress.finish();

    let freed: u64 = removed.iter().map(|entry| entry.freed).sum();
    let trashed: u64 = removed.iter().map(|entry| entry.trashed).sum();
    let failed = removed.iter().any(|entry| !entry.errors.is_empty());

    let mut removal = fsx::Removal {
        freed,
        trashed,
        removed: removed.iter().map(|entry| entry.items).sum(),
        ..Default::default()
    };
    for entry in &removed {
        removal.moves.extend(entry.moves.iter().cloned());
    }
    record_run(ctx, "orphans --clean", disposal, &removal);

    if ctx.json {
        emit(json!({
            "dry_run": ctx.dry_run,
            "disposal": disposal,
            "freed": freed,
            "trashed": trashed,
            "removed": removed,
        }));
        return !failed;
    }

    for entry in &removed {
        for error in &entry.errors {
            p.error(format!("{}: {error}", entry.bundle_id));
        }
    }
    render_disposal(ctx, freed, trashed);
    !failed
}

fn render_leftover(p: &Printer, number: usize, leftover: &orphans::Leftover) {
    p.info(format!(
        "  {:>3}. {:>10}  {:<36} {}",
        number,
        format::size(leftover.bytes),
        format::truncate(&leftover.bundle_id, 36),
        p.dim(&orphans::kinds(leftover))
    ));
}

// ── journal ─────────────────────────────────────────────────────────────────

fn history(ctx: &Ctx, top: usize) -> bool {
    let runs = journal::list();
    let shown = take(&runs, top);

    if ctx.json {
        emit(json!({ "count": runs.len(), "runs": shown }));
        return true;
    }

    let p = &ctx.printer;
    p.heading(&format!("Journal ({} run(s))", runs.len()));
    if runs.is_empty() {
        p.item(p.dim("nothing has been removed yet"));
        return true;
    }

    for run in shown {
        let size = if run.trashed > 0 {
            format!("{} to trash", format::size(run.trashed))
        } else {
            format!("{} freed", format::size(run.freed))
        };
        p.info(format!(
            "  {}  {:<16} {:<22} {:>16}  {}",
            run.id,
            format::datetime(run.epoch),
            format::truncate(&run.command, 22),
            size,
            p.dim(if run.reversible() {
                "undoable"
            } else {
                "permanent"
            })
        ));
    }

    p.info("");
    p.info(p.dim(&format!("  {}", format::tilde(&journal::dir()))));
    p.info(p.dim("  detox undo <ID>   puts a run back"));
    true
}

fn undo_run(ctx: &Ctx, id: Option<&str>) -> bool {
    let p = &ctx.printer;

    let Some(run) = journal::find(id) else {
        p.error(match id {
            Some(wanted) => format!("no run called `{wanted}` — see detox history"),
            None => "nothing in the journal to undo".to_string(),
        });
        return false;
    };

    if !run.reversible() {
        p.error(format!(
            "{} removed {} for good — there is nothing to put back",
            run.id,
            format::size(run.freed)
        ));
        return false;
    }

    if !ctx.json {
        p.heading(&format!(
            "{} — {} · {}",
            run.id,
            format::datetime(run.epoch),
            format::since(run.epoch)
        ));
        p.field("Command", &run.command);
        p.field("Entries", format!("{} item(s)", run.moves.len()));
        p.field("Size", format::size(run.trashed));
    }

    if !ctx.confirm(&format!("Put back {} item(s)?", run.moves.len())) {
        if !ctx.json {
            p.info("Cancelled.");
        }
        return false;
    }

    let result = journal::undo(&run, ctx.dry_run);

    if ctx.json {
        emit(json!({ "dry_run": ctx.dry_run, "run": run.id, "result": result }));
        return result.errors.is_empty();
    }

    p.info("");
    p.success(format!(
        "{} item(s) put back, {}{}",
        result.restored,
        format::size(result.bytes),
        if ctx.dry_run { " [dry run]" } else { "" }
    ));
    if result.gone > 0 {
        p.item(p.dim(&format!(
            "{} no longer in the trash — it was emptied",
            result.gone
        )));
    }
    if result.occupied > 0 {
        p.item(p.dim(&format!(
            "{} left alone: something is at their original path again",
            result.occupied
        )));
    }
    for error in &result.errors {
        p.error(error);
    }
    result.errors.is_empty()
}

// ── memory ──────────────────────────────────────────────────────────────────

/// Takes a snapshot while showing a spinner, since it takes about a second.
fn memory_snapshot(ctx: &Ctx, include_system: bool) -> ram::Snapshot {
    let mut progress = ctx.printer.spinner("Reading processes");
    progress.tick("ps, top, launchd");
    let snapshot = ram::snapshot(include_system);
    progress.finish();
    snapshot
}

fn memory(ctx: &Ctx, top: usize, all: bool, detail: bool, min: u64) -> bool {
    let snapshot = memory_snapshot(ctx, all);
    let groups: Vec<&ram::Group> = snapshot
        .groups
        .iter()
        .filter(|group| group.bytes >= min)
        .collect();
    let shown = take(&groups, top);

    if ctx.json {
        emit(json!({
            "memory": snapshot.memory,
            "swap": snapshot.swap,
            "free_percent": snapshot.free_percent,
            "listed_bytes": snapshot.listed_bytes(),
            "hidden_groups": snapshot.hidden_groups,
            "hidden_bytes": snapshot.hidden_bytes,
            "groups": shown,
        }));
        return true;
    }

    let p = &ctx.printer;
    let memory = &snapshot.memory;

    p.heading("Memory");
    p.field(
        "Physical",
        format!(
            "{} — {} used, {} free, {} inactive",
            format::size(memory.total),
            format::size(memory.used),
            format::size(memory.free),
            format::size(memory.inactive)
        ),
    );
    if snapshot.swap.total > 0 {
        p.field(
            "Swap",
            format!(
                "{} used of {}",
                format::size(snapshot.swap.used),
                format::size(snapshot.swap.total)
            ),
        );
    }
    if let Some(free) = snapshot.free_percent {
        p.field("Pressure", format!("{free}% of memory free"));
    }

    p.heading(&format!(
        "{} ({} group(s) — {})",
        if all {
            "All processes"
        } else {
            "Processes outside macOS"
        },
        groups.len(),
        format::size(groups.iter().map(|group| group.bytes).sum::<u64>())
    ));

    for (index, group) in shown.iter().enumerate() {
        let mut tags = Vec::new();
        if group.autostart {
            tags.push("starts at login".to_string());
        }
        if group.system {
            tags.push("system".to_string());
        }
        tags.push(format!("{} proc.", group.processes.len()));

        p.info(format!(
            "  {:>3}. {:>10}  {:<34} {}",
            index + 1,
            format::size(group.bytes),
            format::truncate(&group.name, 34),
            p.dim(&tags.join(" · "))
        ));

        if detail {
            const DETAIL_LIMIT: usize = 8;
            for process in group.processes.iter().take(DETAIL_LIMIT) {
                p.info(format!(
                    "                   {:>10}  {}",
                    format::size(process.bytes),
                    p.dim(&format!("{} [{}]", process.name, process.pid))
                ));
            }
            if group.processes.len() > DETAIL_LIMIT {
                p.info(p.dim(&format!(
                    "                   … and {} smaller processes",
                    group.processes.len() - DETAIL_LIMIT
                )));
            }
        }
    }

    if shown.len() < groups.len() {
        p.info(p.dim(&format!(
            "  … and {} group(s) below {}",
            groups.len() - shown.len(),
            format::size(shown.last().map_or(0, |group| group.bytes))
        )));
    }

    if snapshot.hidden_groups > 0 {
        p.info("");
        p.info(p.dim(&format!(
            "  {} macOS/Apple component(s) hidden ({}) — see --all",
            snapshot.hidden_groups,
            format::size(snapshot.hidden_bytes)
        )));
    }

    p.info("");
    p.info(p.dim("  detox inspect <number|name>   detail the processes of one application"));
    p.info(p.dim("  detox kill <number|name>      stop every process of one application"));
    if shown.iter().any(|group| group.autostart) {
        p.info(p.dim(
            "  \"starts at login\" = launched by a launchd agent; detox agents disable <label>",
        ));
    }

    ram::remember(shown);
    true
}

// ── naming an application ───────────────────────────────────────────────────

/// Turns a number printed by `ram` back into an application name.
fn target_query(p: &Printer, raw: &str) -> Option<String> {
    match raw.trim().parse::<usize>() {
        Ok(index) => match ram::recall(index) {
            Some(name) => Some(name),
            None => {
                p.error(format!("no group #{index} in memory — run detox ram first"));
                None
            }
        },
        Err(_) => Some(raw.trim().to_string()),
    }
}

/// Finds the targeted group, explaining any failure.
fn locate<'a>(p: &Printer, snapshot: &'a ram::Snapshot, query: &str) -> Option<&'a ram::Group> {
    match ram::find(&snapshot.groups, query) {
        ram::Lookup::One(group) => Some(group),
        ram::Lookup::Ambiguous(names) => {
            p.error(format!("`{query}` matches several applications:"));
            for name in names {
                p.item(name);
            }
            p.item(p.dim("narrow the name, or use the number shown by detox ram"));
            None
        }
        ram::Lookup::None => {
            p.error(format!("no running application named `{query}`."));
            None
        }
    }
}

// ── detail of one application ───────────────────────────────────────────────

fn inspect(ctx: &Ctx, raw_query: &str, short: bool) -> bool {
    let p = &ctx.printer;
    let Some(query) = target_query(p, raw_query) else {
        return false;
    };

    let snapshot = memory_snapshot(ctx, true);
    let Some(group) = locate(p, &snapshot, &query) else {
        return false;
    };

    let mut group = group.clone();
    ram::with_command_lines(&mut group);

    if ctx.json {
        emit(json!({ "group": group }));
        return true;
    }

    let cpu: f64 = group.processes.iter().map(|process| process.cpu).sum();

    p.heading(&group.name);
    if let Some(bundle) = &group.bundle {
        p.field("Bundle", format::tilde(bundle));
    }
    p.field(
        "Memory",
        format!(
            "{} — {} processes",
            format::size(group.bytes),
            group.processes.len()
        ),
    );
    p.field("CPU", format!("{cpu:.1}% (average since launch)"));
    if let Some(process) = group.processes.first() {
        p.field("User", &process.user);
    }
    if !group.agents.is_empty() {
        p.field("Starts at login", group.agents.join(", "));
    }
    if group.system {
        p.field("Origin", "macOS / Apple component");
    }

    p.info("");
    let (roots, children) = ram::tree(&group);
    for root in roots {
        render_process(p, root, &children, "  ", "", short);
    }

    if !group.agents.is_empty() {
        p.info("");
        p.info(p.dim(&format!(
            "  detox agents disable {}   stops it from starting on its own",
            group.agents[0]
        )));
    }
    true
}

/// Renders a process and its descendants inside the group.
fn render_process(
    p: &Printer,
    process: &ram::Process,
    children: &std::collections::HashMap<u32, Vec<&ram::Process>>,
    prefix: &str,
    connector: &str,
    short: bool,
) {
    let head = format!(
        "{prefix}{connector}{:>10}  {} [{}]",
        format::size(process.bytes),
        format::truncate(&process.name, 30),
        process.pid
    );
    let padding = 64usize.saturating_sub(head.chars().count());
    p.info(format!(
        "{head}{}{}",
        " ".repeat(padding),
        p.dim(&format!(
            "{:>5.1}% · {}",
            process.cpu,
            format::elapsed(&process.elapsed)
        ))
    ));

    // The branch keeps going under every child but the last one.
    let continuation = match connector.chars().next() {
        None => "  ",
        Some('└') => "   ",
        Some(_) => "│  ",
    };
    let child_prefix = format!("{prefix}{continuation}");

    if !short {
        if let Some(arguments) = process_arguments(process) {
            p.info(format!(
                "{child_prefix}{:>12}{}",
                "",
                p.dim(&format::truncate(&arguments, 88))
            ));
        }
    }

    let kids = children.get(&process.pid).map(Vec::as_slice).unwrap_or(&[]);
    for (index, child) in kids.iter().enumerate() {
        let last = index + 1 == kids.len();
        render_process(
            p,
            child,
            children,
            &child_prefix,
            if last { "└─ " } else { "├─ " },
            short,
        );
    }
}

/// Arguments of a process, without the executable itself.
fn process_arguments(process: &ram::Process) -> Option<String> {
    let full = process.args.as_deref()?;
    let rest = full
        .strip_prefix(&process.path)
        .or_else(|| full.split_once(' ').map(|(_, rest)| rest))
        .unwrap_or_default()
        .trim();

    (!rest.is_empty()).then(|| rest.to_string())
}

// ── stopping an application ─────────────────────────────────────────────────

fn kill(ctx: &Ctx, query: &str, force: bool, allow_system: bool) -> bool {
    let p = &ctx.printer;

    // A number refers to the name shown by the last `detox ram`.
    let Some(query) = target_query(p, query) else {
        return false;
    };

    let snapshot = memory_snapshot(ctx, true);
    let Some(group) = locate(p, &snapshot, &query) else {
        return false;
    };

    if group.system && !allow_system {
        p.error(format!(
            "{} is a macOS component — pass --system to stop it anyway.",
            group.name
        ));
        return false;
    }

    let ancestors = ram::own_ancestors(&snapshot.groups);
    let suicidal = group
        .processes
        .iter()
        .any(|process| ancestors.contains(&process.pid));

    if !ctx.json {
        p.heading(&format!(
            "{} — {} processes, {}",
            group.name,
            group.processes.len(),
            format::size(group.bytes)
        ));
        for process in group.processes.iter().take(8) {
            p.info(format!(
                "  {:>10}  {}",
                format::size(process.bytes),
                p.dim(&format!("{} [{}]", process.name, process.pid))
            ));
        }
        if group.processes.len() > 8 {
            p.info(p.dim(&format!(
                "  … and {} smaller processes",
                group.processes.len() - 8
            )));
        }
    }

    if suicidal {
        p.warn("this group holds the terminal running detox: the command will be cut short too.");
    }
    if force {
        p.warn("SIGKILL: applications get no chance to save.");
    }

    if !ctx.confirm(&format!(
        "Stop {} ({} processes)?",
        group.name,
        group.processes.len()
    )) {
        if !ctx.json {
            p.info("Cancelled.");
        }
        return false;
    }

    let report = ram::kill(group, force, ctx.dry_run);

    if ctx.json {
        emit(json!({ "dry_run": ctx.dry_run, "result": report }));
        return report.survived.is_empty() && report.errors.is_empty();
    }

    p.info("");
    if report.terminated.is_empty() {
        p.error(format!("{} — no process stopped", report.group));
    } else {
        let summary = format!(
            "{} — {} process(es) stopped, {} freed",
            report.group,
            report.terminated.len(),
            format::size(report.bytes)
        );
        if ctx.dry_run {
            p.success(format!("{summary} [dry run]"));
        } else {
            p.success(summary);
        }
    }

    for error in &report.errors {
        p.item(p.dim(error));
    }
    if !report.survived.is_empty() {
        p.warn(format!(
            "{} process(es) still alive: {} — try again with --force",
            report.survived.len(),
            report
                .survived
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    report.errors.is_empty() && report.survived.is_empty() && !report.terminated.is_empty()
}

// ── agents ──────────────────────────────────────────────────────────────────

fn list_agents(ctx: &Ctx, third_party_only: bool, scope: Option<Scope>) -> bool {
    let all = agents::collect();
    let selected: Vec<&Agent> = all
        .iter()
        .filter(|agent| !third_party_only || !agent.apple)
        .filter(|agent| scope.is_none_or(|wanted| agent.scope == wanted))
        .collect();

    if ctx.json {
        emit(json!({ "count": selected.len(), "agents": selected }));
        return true;
    }

    let p = &ctx.printer;
    for scope in Scope::ALL {
        let group: Vec<&&Agent> = selected
            .iter()
            .filter(|agent| agent.scope == scope)
            .collect();
        if group.is_empty() {
            continue;
        }
        p.heading(scope.label());
        for agent in group {
            let origin = if agent.apple { "Apple" } else { "third-party" };
            let state = if agent.loaded { "loaded" } else { "not loaded" };
            p.info(format!(
                "  {:<52} {}",
                agent.label,
                p.dim(&format!("{origin} · {state}"))
            ));
        }
    }

    let (apple, third_party) = agents::summary(&all);
    p.info("");
    p.info(format!(
        "  {third_party} third-party agent(s), {apple} Apple"
    ));
    true
}

fn act_on_agents(ctx: &Ctx, action: Action, selection: Selection) -> bool {
    let all = agents::collect();

    let mut unknown = Vec::new();
    let selected: Vec<&Agent> = if selection.all_third_party {
        all.iter()
            .filter(|agent| !agent.apple)
            .filter(|agent| selection.scope.is_none_or(|wanted| agent.scope == wanted))
            .collect()
    } else {
        let mut found = Vec::new();
        for label in &selection.labels {
            match all.iter().find(|agent| &agent.label == label) {
                Some(agent) => found.push(agent),
                None => unknown.push(label.clone()),
            }
        }
        found
    };

    for label in &unknown {
        ctx.printer.warn(format!("unknown agent: {label}"));
    }

    if selected.is_empty() {
        ctx.printer.error("no matching agent.");
        return false;
    }

    let apple_count = selected.iter().filter(|agent| agent.apple).count();
    if apple_count > 0 {
        ctx.printer.warn(format!(
            "{apple_count} Apple agent(s) skipped: detox never touches system components."
        ));
    }

    let verb = match action {
        Action::Disable => "Disable",
        Action::Enable => "Enable",
        Action::Remove => "Permanently remove",
    };
    if !ctx.confirm(&format!("{verb} {} agent(s)?", selected.len())) {
        if !ctx.json {
            ctx.printer.info("Cancelled.");
        }
        return false;
    }

    let mut progress = ctx.printer.bar("Applying", selected.len());
    let outcomes: Vec<Outcome> = selected
        .iter()
        .map(|agent| {
            progress.tick(&agent.label);
            let outcome = agents::apply(agent, action, ctx);
            progress.advance(&agent.label);
            outcome
        })
        .collect();
    progress.finish();

    render_outcomes(ctx, &outcomes)
}

// ── system ──────────────────────────────────────────────────────────────────

fn sys(ctx: &Ctx, command: SysCommand) -> bool {
    let needs_confirmation = !matches!(command, SysCommand::Updates);
    let question = match command {
        SysCommand::Dns => "Flush the DNS cache?",
        SysCommand::Spotlight => "Rebuild the Spotlight index?",
        SysCommand::Memory => "Free inactive memory?",
        SysCommand::Snapshots => "Purge local Time Machine snapshots?",
        SysCommand::Updates => "",
    };

    if needs_confirmation && !ctx.confirm(question) {
        if !ctx.json {
            ctx.printer.info("Cancelled.");
        }
        return false;
    }

    let mut progress = ctx.printer.spinner("Working");
    progress.tick(question.trim_end_matches('?'));
    let outcome = match command {
        SysCommand::Dns => maintenance::flush_dns(ctx),
        SysCommand::Spotlight => maintenance::reindex_spotlight(ctx),
        SysCommand::Memory => maintenance::purge_memory(ctx),
        SysCommand::Snapshots => maintenance::thin_snapshots(ctx),
        SysCommand::Updates => maintenance::check_updates(),
    };
    progress.finish();

    render_outcomes(ctx, std::slice::from_ref(&outcome))
}

fn render_outcomes(ctx: &Ctx, outcomes: &[Outcome]) -> bool {
    let failed = outcomes.iter().any(|outcome| outcome.status.is_failure());

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
