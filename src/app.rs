//! Application layer: runs commands and formats their results.

use std::path::PathBuf;

use clap::CommandFactory;
use serde_json::{Value, json};

use crate::cli::{
    AgentCommand, Cli, Command, DevFilter, FileCommand, Selection, SysCommand, TargetArg,
};
use crate::format;
use crate::sys::fsx;
use crate::sys::machine::Machine;
use crate::task::agents::{self, Action, Agent, Scope};
use crate::task::clean::{self, Target};
use crate::task::{Ctx, Outcome, Status, dev, maintenance, ram, scan};
use crate::ui::Printer;

/// Runs the requested command. Returns `false` when something failed.
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
        Command::Files { command } => match command {
            FileCommand::Large {
                min,
                top,
                path,
                depth,
            } => large_files(&ctx, min, top, path, depth),
            FileCommand::Dev { filter, top } => dev_residue(&ctx, &filter, top, None),
            FileCommand::Clean { filter, native } => dev_residue(&ctx, &filter, 0, Some(native)),
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
    p.info("");
    p.info(p.dim("  detox-mac ram         see what is eating memory"));
    p.info(p.dim("  detox-mac files dev   find build residue in your projects"));
    p.info(p.dim("  detox-mac scan all    measure everything"));
    p.info(p.dim("  detox-mac clean all   clean everything"));
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
            p.dim(&format!("{} item(s)", measure.items))
        )),
    }
}

fn scan_targets(ctx: &Ctx, targets: Vec<Target>) -> bool {
    let measures = measure_targets(ctx, &targets);
    let total: u64 = measures.iter().map(|measure| measure.bytes).sum();

    if ctx.json {
        emit(json!({ "total": total, "targets": measures }));
        return true;
    }

    let p = &ctx.printer;
    p.heading("Reclaimable space");
    for measure in &measures {
        render_measure(p, measure);
    }
    p.info(p.bold(&format!("  {:<28} {:>10}", "Total", format::size(total))));
    true
}

// ── clean ───────────────────────────────────────────────────────────────────

fn clean_targets(ctx: &Ctx, targets: Vec<Target>) -> bool {
    let labels: Vec<&str> = targets.iter().map(|target| target.slug()).collect();
    if !ctx.confirm(&format!("Clean: {}?", labels.join(", "))) {
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
    let failed = results.iter().any(|result| result.status.is_failure());

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
            "Reclaimable:"
        } else {
            "Freed:"
        },
        p.accent(&format::size(freed))
    ));
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

fn dev_residue(ctx: &Ctx, filter: &DevFilter, top: usize, remove: Option<bool>) -> bool {
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
        p.info(p.dim("  detox-mac files clean   removes them (your next build rebuilds them)"));
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
        for entry in residue.iter().take(15) {
            render_residue(p, entry);
            if native {
                if let Some(command) = dev::planned_command(entry) {
                    p.item(p.dim(&format!("via {command}")));
                }
            }
        }
        if residue.len() > 15 {
            p.info(p.dim(&format!("  … and {} more", residue.len() - 15)));
        }
    }

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

    let freed: u64 = removed.iter().map(|entry| entry.bytes).sum();
    let failed = removed.iter().filter(|entry| entry.error.is_some()).count();

    if ctx.json {
        emit(json!({ "dry_run": ctx.dry_run, "freed": freed, "removed": removed }));
        return failed == 0;
    }

    p.info("");
    let summary = format!(
        "{} director{} removed, {} freed",
        removed.len() - failed,
        if removed.len() - failed == 1 {
            "y"
        } else {
            "ies"
        },
        format::size(freed)
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
    failed == 0
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
    p.info(p.dim("  detox-mac inspect <number|name>   detail the processes of one application"));
    p.info(p.dim("  detox-mac kill <number|name>      stop every process of one application"));
    if shown.iter().any(|group| group.autostart) {
        p.info(p.dim(
            "  \"starts at login\" = launched by a launchd agent; detox-mac agents disable <label>",
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
                p.error(format!(
                    "no group #{index} in memory — run detox-mac ram first"
                ));
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
            p.item(p.dim("narrow the name, or use the number shown by detox-mac ram"));
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
            "  detox-mac agents disable {}   stops it from starting on its own",
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

    // A number refers to the name shown by the last `detox-mac ram`.
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
        p.warn(
            "this group holds the terminal running detox-mac: the command will be cut short too.",
        );
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
            "{apple_count} Apple agent(s) skipped: detox-mac never touches system components."
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
