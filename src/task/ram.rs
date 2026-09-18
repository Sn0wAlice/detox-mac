//! Memory inspection, the way Activity Monitor does it.
//!
//! Processes are grouped by parent application: the many helpers of an Electron
//! app or a browser count as a single line.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use super::agents;
use crate::sys::{cmd, machine};

/// A live process.
#[derive(Debug, Clone, Serialize)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub user: String,
    /// Executable name.
    pub name: String,
    pub path: String,
    /// Memory footprint (`top`), falling back to resident size (`ps`).
    pub bytes: u64,
    /// Average CPU share since the process started.
    pub cpu: f64,
    /// Time since launch, as `ps` reports it (`3-04:12:33`).
    pub elapsed: String,
    /// Full command line, filled in by `inspect` only.
    pub args: Option<String>,
}

/// A set of processes belonging to the same application.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub name: String,
    /// Originating `.app` bundle, when there is one.
    pub bundle: Option<PathBuf>,
    pub bytes: u64,
    /// A macOS component or an Apple application.
    pub system: bool,
    /// Started automatically by a `launchd` agent.
    pub autostart: bool,
    /// Labels of the `launchd` agents that start this application.
    pub agents: Vec<String>,
    pub processes: Vec<Process>,
}

/// Snapshot of memory at a given moment.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub memory: machine::Memory,
    pub swap: machine::Swap,
    /// Percentage of free memory according to `memory_pressure`.
    pub free_percent: Option<u8>,
    pub groups: Vec<Group>,
    /// System groups left out of the list (when they were not asked for).
    pub hidden_groups: usize,
    pub hidden_bytes: u64,
}

impl Snapshot {
    /// Total memory of the listed groups.
    pub fn listed_bytes(&self) -> u64 {
        self.groups.iter().map(|g| g.bytes).sum()
    }
}

/// Builds the current snapshot.
///
/// `include_system` keeps Apple processes and system daemons.
pub fn snapshot(include_system: bool) -> Snapshot {
    let processes = running_processes();
    let by_pid: HashMap<u32, usize> = processes
        .iter()
        .enumerate()
        .map(|(index, process)| (process.pid, index))
        .collect();

    let autostart = autostart_groups();
    let mut groups: HashMap<String, Group> = HashMap::new();

    for process in &processes {
        let anchor = resolve_group(process, &processes, &by_pid);
        let entry = groups.entry(anchor.name.clone()).or_insert_with(|| Group {
            name: anchor.name.clone(),
            bundle: anchor.bundle.clone(),
            bytes: 0,
            system: anchor.system,
            autostart: autostart.contains_key(&anchor.name),
            agents: autostart.get(&anchor.name).cloned().unwrap_or_default(),
            processes: Vec::new(),
        });
        entry.bytes += process.bytes;
        entry.processes.push(process.clone());
    }

    let mut groups: Vec<Group> = groups.into_values().collect();
    for group in &mut groups {
        group.processes.sort_by_key(|p| std::cmp::Reverse(p.bytes));
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.bytes));

    let mut hidden_groups = 0;
    let mut hidden_bytes = 0;
    if !include_system {
        groups.retain(|group| {
            if group.system {
                hidden_groups += 1;
                hidden_bytes += group.bytes;
            }
            !group.system
        });
    }

    Snapshot {
        memory: machine::Memory::collect(),
        swap: machine::Swap::collect(),
        free_percent: free_percent(),
        groups,
        hidden_groups,
        hidden_bytes,
    }
}

/// Application a process should be attached to.
struct Anchor {
    name: String,
    bundle: Option<PathBuf>,
    system: bool,
}

/// Attaches a process to its parent application.
///
/// The parent chain is walked as long as the process does not belong to a bundle
/// itself, so `rust-analyzer` ends up under its editor.
/// A system parent never absorbs a third-party process, otherwise everything
/// would end up grouped under `launchd`.
fn resolve_group(process: &Process, processes: &[Process], by_pid: &HashMap<u32, usize>) -> Anchor {
    if let Some((name, bundle)) = bundle_of(&process.path) {
        return Anchor {
            name,
            bundle: Some(bundle),
            system: is_system(&process.path),
        };
    }

    let mut current = process.ppid;
    let mut seen = HashSet::new();
    while current > 1 && seen.insert(current) {
        let Some(&index) = by_pid.get(&current) else {
            break;
        };
        let parent = &processes[index];
        if let Some((name, bundle)) = bundle_of(&parent.path) {
            if !is_system(&parent.path) {
                return Anchor {
                    name,
                    bundle: Some(bundle),
                    system: false,
                };
            }
            break;
        }
        current = parent.ppid;
    }

    Anchor {
        name: process.name.clone(),
        bundle: None,
        system: is_system(&process.path),
    }
}

/// Outermost `.app` bundle of the path, and its name.
fn bundle_of(path: &str) -> Option<(String, PathBuf)> {
    let mut bundle = PathBuf::new();
    for component in Path::new(path).components() {
        bundle.push(component);
        let name = component.as_os_str().to_string_lossy();
        if let Some(stem) = name.strip_suffix(".app") {
            return Some((stem.to_string(), bundle));
        }
    }
    None
}

/// Locations belonging to macOS or to Apple.
const SYSTEM_PREFIXES: &[&str] = &[
    "/System/",
    "/usr/",
    "/bin/",
    "/sbin/",
    "/Library/Apple/",
    "/Library/CoreMediaIO/",
    "/Library/PrivilegedHelperTools/com.apple.",
];

/// A process is "system" when it comes from macOS itself.
fn is_system(path: &str) -> bool {
    !path.starts_with('/')
        || path.contains("/Cryptexes/")
        || SYSTEM_PREFIXES
            .iter()
            .any(|prefix| path.starts_with(prefix))
}

/// Lists processes with their memory footprint.
fn running_processes() -> Vec<Process> {
    let Ok(output) = cmd::run(
        "ps",
        &["-axwwo", "pid=,ppid=,rss=,%cpu=,etime=,user=,comm="],
    ) else {
        return Vec::new();
    };
    let footprints = footprints();

    output
        .stdout
        .lines()
        .filter_map(|line| {
            let (fields, path) = split_fields(line, 6)?;
            let pid: u32 = fields[0].parse().ok()?;
            let resident = fields[2].parse::<u64>().unwrap_or(0) * 1024;

            Some(Process {
                pid,
                ppid: fields[1].parse().unwrap_or(0),
                cpu: fields[3].parse().unwrap_or(0.0),
                elapsed: fields[4].to_string(),
                user: fields[5].to_string(),
                args: None,
                name: Path::new(path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                path: path.to_string(),
                bytes: footprints.get(&pid).copied().unwrap_or(resident),
            })
        })
        .collect()
}

/// Splits `count` leading fields off a line and returns the rest untouched.
fn split_fields(line: &str, count: usize) -> Option<(Vec<&str>, &str)> {
    let mut fields = Vec::with_capacity(count);
    let mut rest = line.trim_start();

    for _ in 0..count {
        let end = rest.find(char::is_whitespace)?;
        fields.push(&rest[..end]);
        rest = rest[end..].trim_start();
    }

    (!rest.is_empty()).then_some((fields, rest))
}

/// Memory footprint per PID, the figure Activity Monitor shows.
fn footprints() -> HashMap<u32, u64> {
    let Ok(output) = cmd::run("top", &["-l", "1", "-n", "20000", "-stats", "pid,mem"]) else {
        return HashMap::new();
    };

    output
        .stdout
        .lines()
        .skip_while(|line| !line.trim_start().starts_with("PID"))
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            Some((pid, parse_top_size(fields.next()?)?))
        })
        .collect()
}

/// Parses a size printed by `top`: `6257K`, `726M`, `1.2G`, `512B`.
fn parse_top_size(input: &str) -> Option<u64> {
    let text = input.trim_end_matches(['+', '-']);
    let split = text.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (number, unit) = text.split_at(split);
    let value: f64 = number.parse().ok()?;

    let multiplier = match unit {
        "B" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        "T" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };

    Some((value * multiplier) as u64)
}

/// Percentage of free memory according to `memory_pressure`.
fn free_percent() -> Option<u8> {
    let output = cmd::run("memory_pressure", &["-Q"]).ok()?;
    output
        .stdout
        .lines()
        .find_map(|line| line.split("free percentage:").nth(1))
        .and_then(|value| value.trim().trim_end_matches('%').parse().ok())
}

/// Groups started automatically, with the label of the agent responsible.
fn autostart_groups() -> HashMap<String, Vec<String>> {
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();

    for agent in agents::collect().iter().filter(|agent| !agent.apple) {
        let Some(program) = agent_program(&agent.path) else {
            continue;
        };
        let name = match bundle_of(&program) {
            Some((name, _)) => name,
            None => Path::new(&program)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        };
        groups.entry(name).or_default().push(agent.label.clone());
    }

    groups
}

/// Full command lines, per PID.
///
/// Kept apart from the main listing: only `inspect` needs it.
pub fn command_lines() -> HashMap<u32, String> {
    let Ok(output) = cmd::run("ps", &["-axwwo", "pid=,args="]) else {
        return HashMap::new();
    };

    output
        .stdout
        .lines()
        .filter_map(|line| {
            let (fields, args) = split_fields(line, 1)?;
            Some((fields[0].parse().ok()?, args.to_string()))
        })
        .collect()
}

/// Fills in the command line of every process of a group.
pub fn with_command_lines(group: &mut Group) {
    let lines = command_lines();
    for process in &mut group.processes {
        process.args = lines.get(&process.pid).cloned();
    }
}

/// Parent → children tree inside a group.
///
/// Roots are the processes whose parent lies outside the group.
pub fn tree(group: &Group) -> (Vec<&Process>, HashMap<u32, Vec<&Process>>) {
    let pids: HashSet<u32> = group.processes.iter().map(|p| p.pid).collect();
    let mut roots = Vec::new();
    let mut children: HashMap<u32, Vec<&Process>> = HashMap::new();

    for process in &group.processes {
        if pids.contains(&process.ppid) {
            children.entry(process.ppid).or_default().push(process);
        } else {
            roots.push(process);
        }
    }

    (roots, children)
}

/// Executable started by an agent, read from its `.plist` file.
fn agent_program(plist: &Path) -> Option<String> {
    let args = [
        std::ffi::OsStr::new("-convert"),
        std::ffi::OsStr::new("json"),
        std::ffi::OsStr::new("-o"),
        std::ffi::OsStr::new("-"),
        plist.as_os_str(),
    ];
    let output = cmd::run("plutil", &args).ok()?;
    let value: serde_json::Value = serde_json::from_str(&output.stdout).ok()?;

    value["Program"]
        .as_str()
        .or_else(|| value["ProgramArguments"][0].as_str())
        .map(str::to_string)
}

// ── naming a group ──────────────────────────────────────────────────────────

/// Result of looking a group up by name.
pub enum Lookup<'a> {
    /// Exactly one group matches.
    One(&'a Group),
    /// Several matches: the query must be narrowed.
    Ambiguous(Vec<&'a str>),
    /// No match at all.
    None,
}

/// Match test between a group name and a query.
type Matcher = fn(&str, &str) -> bool;

/// From the strictest to the most permissive.
///
/// Case matters on the first pass: that is what tells the `Claude` application
/// from the `claude` executable, and makes the `ram` numbers dependable.
const MATCHERS: [Matcher; 4] = [
    |name, query| name == query,
    |name, query| name.to_lowercase() == query.to_lowercase(),
    |name, query| name.to_lowercase().contains(&query.to_lowercase()),
    matches_words,
];

/// Finds a group from an approximate name.
///
/// In order: exact name, name containing the query, then a letter-by-letter
/// word match (`vs code` finds `Visual Studio Code`).
pub fn find<'a>(groups: &'a [Group], query: &str) -> Lookup<'a> {
    let needle = query.trim();

    for matches in MATCHERS {
        let found: Vec<&Group> = groups
            .iter()
            .filter(|group| matches(&group.name, needle))
            .collect();

        match found.len() {
            0 => continue,
            1 => return Lookup::One(found[0]),
            _ => return Lookup::Ambiguous(found.iter().map(|g| g.name.as_str()).collect()),
        }
    }

    Lookup::None
}

/// `vs code` and `vscode` both find `Visual Studio Code`.
///
/// The words of the name consume the query letter by letter, in order: each word
/// eats the longest common prefix, possibly none at all.
fn matches_words(name: &str, query: &str) -> bool {
    let needle: String = query
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();

    if needle.is_empty() {
        return false;
    }

    let name = name.to_lowercase();
    let mut rest = needle.as_str();

    for word in name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let consumed: usize = rest
            .chars()
            .zip(word.chars())
            .take_while(|(from_query, from_name)| from_query == from_name)
            .map(|(c, _)| c.len_utf8())
            .sum();

        rest = &rest[consumed..];
        if rest.is_empty() {
            return true;
        }
    }

    false
}

// ── numbers remembered between two commands ─────────────────────────────────

/// File where `ram` records the list it printed, so that `kill 3` works.
fn state_path() -> PathBuf {
    std::env::temp_dir().join("detox-mac-ram.json")
}

/// Remembers the groups that were displayed, in order.
pub fn remember(groups: &[&Group]) {
    let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    if let Ok(json) = serde_json::to_string(&names) {
        let _ = std::fs::write(state_path(), json);
    }
}

/// Name of the group shown at this position by the last `detox-mac ram`.
pub fn recall(index: usize) -> Option<String> {
    let raw = std::fs::read_to_string(state_path()).ok()?;
    let names: Vec<String> = serde_json::from_str(&raw).ok()?;
    index
        .checked_sub(1)
        .and_then(|position| names.get(position))
        .cloned()
}

// ── stopping an application ─────────────────────────────────────────────────

/// Report of a group shutdown.
#[derive(Debug, Clone, Serialize)]
pub struct Killed {
    pub group: String,
    pub signal: &'static str,
    /// Memory held by the targeted processes.
    pub bytes: u64,
    /// Processes that actually stopped.
    pub terminated: Vec<u32>,
    /// Processes still alive after the signal.
    pub survived: Vec<u32>,
    pub errors: Vec<String>,
}

/// Sends a signal to every process of a group.
///
/// Root processes go first: the helpers of an Electron app then shut down on
/// their own instead of showing a crash window.
pub fn kill(group: &Group, force: bool, dry_run: bool) -> Killed {
    let signal = if force { "-KILL" } else { "-TERM" };
    let own = std::process::id();
    let pids: HashSet<u32> = group.processes.iter().map(|p| p.pid).collect();

    let (roots, children): (Vec<&Process>, Vec<&Process>) = group
        .processes
        .iter()
        .filter(|process| process.pid != own)
        .partition(|process| !pids.contains(&process.ppid));

    let mut report = Killed {
        group: group.name.clone(),
        signal: if force { "SIGKILL" } else { "SIGTERM" },
        bytes: group.bytes,
        terminated: Vec::new(),
        survived: Vec::new(),
        errors: Vec::new(),
    };

    for process in roots.into_iter().chain(children) {
        if dry_run {
            report.terminated.push(process.pid);
            continue;
        }

        match cmd::run("kill", &[signal, &process.pid.to_string()]) {
            Ok(_) => report.terminated.push(process.pid),
            // A child already gone with its parent is not an error.
            Err(err) if err.contains("No such process") => report.terminated.push(process.pid),
            Err(err) => report
                .errors
                .push(format!("{} [{}] : {err}", process.name, process.pid)),
        }
    }

    if !dry_run && !report.terminated.is_empty() {
        std::thread::sleep(Duration::from_millis(500));
        report.survived = report
            .terminated
            .iter()
            .copied()
            .filter(|pid| is_alive(*pid))
            .collect();
        report
            .terminated
            .retain(|pid| !report.survived.contains(pid));
    }

    report
}

/// True when the process still exists.
fn is_alive(pid: u32) -> bool {
    cmd::run("kill", &["-0", &pid.to_string()]).is_ok()
}

/// Ancestors of the current process, so we never kill ourselves unknowingly.
pub fn own_ancestors(groups: &[Group]) -> HashSet<u32> {
    let parents: HashMap<u32, u32> = groups
        .iter()
        .flat_map(|group| group.processes.iter())
        .map(|process| (process.pid, process.ppid))
        .collect();

    let mut ancestors = HashSet::new();
    let mut current = std::process::id();
    while let Some(&parent) = parents.get(&current) {
        if parent <= 1 || !ancestors.insert(parent) {
            break;
        }
        current = parent;
    }
    ancestors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_outermost_bundle() {
        let path = "/Applications/Claude.app/Contents/Frameworks/Claude Helper (Renderer).app/Contents/MacOS/Claude Helper";
        let (name, bundle) = bundle_of(path).unwrap();
        assert_eq!(name, "Claude");
        assert_eq!(bundle, PathBuf::from("/Applications/Claude.app"));
    }

    #[test]
    fn plain_binaries_have_no_bundle() {
        assert!(bundle_of("/opt/homebrew/bin/node").is_none());
    }

    #[test]
    fn recognises_system_paths() {
        assert!(is_system("/usr/libexec/logd"));
        assert!(is_system(
            "/System/Applications/Siri AI.app/Contents/MacOS/Siri AI"
        ));
        assert!(is_system(
            "/System/Volumes/Preboot/Cryptexes/App/System/Applications/Safari.app/Contents/MacOS/Safari"
        ));
        assert!(!is_system(
            "/Applications/Discord.app/Contents/MacOS/Discord"
        ));
        assert!(!is_system("/opt/homebrew/bin/node"));
    }

    #[test]
    fn splits_paths_containing_spaces() {
        let line = "  79148 78801 654688 alice  /Applications/Claude.app/Contents/MacOS/Claude Helper (Renderer)";
        let (fields, path) = split_fields(line, 4).unwrap();
        assert_eq!(fields, vec!["79148", "78801", "654688", "alice"]);
        assert_eq!(
            path,
            "/Applications/Claude.app/Contents/MacOS/Claude Helper (Renderer)"
        );
    }

    fn group(name: &str) -> Group {
        Group {
            name: name.to_string(),
            bundle: None,
            bytes: 0,
            system: false,
            autostart: false,
            agents: Vec::new(),
            processes: Vec::new(),
        }
    }

    #[test]
    fn exact_case_wins_over_fuzzy_matches() {
        let groups = [group("Claude"), group("claude"), group("Discord")];

        assert!(matches!(find(&groups, "Claude"), Lookup::One(g) if g.name == "Claude"));
        assert!(matches!(find(&groups, "claude"), Lookup::One(g) if g.name == "claude"));
        assert!(matches!(find(&groups, "disc"), Lookup::One(g) if g.name == "Discord"));
        assert!(matches!(find(&groups, "CLAUDE"), Lookup::Ambiguous(_)));
        assert!(matches!(find(&groups, "firefox"), Lookup::None));
    }

    #[test]
    fn matches_names_word_by_word() {
        assert!(matches_words("Visual Studio Code", "vs code"));
        assert!(matches_words("Visual Studio Code", "vscode"));
        assert!(matches_words("Visual Studio Code", "code"));
        assert!(matches_words("Visual Studio Code", "visual"));
        assert!(matches_words("Docker Desktop", "docker"));
        assert!(!matches_words("Visual Studio Code", "vs terminal"));
        assert!(!matches_words("Spotify", "vs"));
        assert!(!matches_words("Discord", ""));
    }

    #[test]
    fn parses_top_sizes() {
        assert_eq!(parse_top_size("6257K"), Some(6257 * 1024));
        assert_eq!(parse_top_size("726M+"), Some(726 * 1024 * 1024));
        assert_eq!(parse_top_size("1.5G"), Some(1024 * 1024 * 1024 * 3 / 2));
        assert_eq!(parse_top_size("42"), None);
    }
}
