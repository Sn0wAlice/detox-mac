//! Inspection de la mémoire vive, à la manière du Moniteur d'activité.
//!
//! Les processus sont regroupés par application mère : les nombreux helpers
//! d'un Electron ou d'un navigateur comptent pour une seule ligne.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use super::agents;
use crate::sys::{cmd, machine};

/// Un processus vivant.
#[derive(Debug, Clone, Serialize)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub user: String,
    /// Nom de l'exécutable.
    pub name: String,
    pub path: String,
    /// Empreinte mémoire (`top`), sinon la taille résidente (`ps`).
    pub bytes: u64,
    /// Part de CPU moyenne depuis le lancement du processus.
    pub cpu: f64,
    /// Durée depuis le lancement, telle que `ps` la donne (`3-04:12:33`).
    pub elapsed: String,
    /// Ligne de commande complète, renseignée par `inspect` seulement.
    pub args: Option<String>,
}

/// Un ensemble de processus rattachés à la même application.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub name: String,
    /// Bundle `.app` d'origine, quand il y en a un.
    pub bundle: Option<PathBuf>,
    pub bytes: u64,
    /// Composant du système ou application Apple.
    pub system: bool,
    /// Lancé automatiquement par un agent `launchd`.
    pub autostart: bool,
    /// Labels des agents `launchd` qui démarrent cette application.
    pub agents: Vec<String>,
    pub processes: Vec<Process>,
}

/// Photographie de la mémoire à un instant donné.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub memory: machine::Memory,
    pub swap: machine::Swap,
    /// Pourcentage de mémoire libre d'après `memory_pressure`.
    pub free_percent: Option<u8>,
    pub groups: Vec<Group>,
    /// Groupes système écartés de la liste (quand ils ne sont pas demandés).
    pub hidden_groups: usize,
    pub hidden_bytes: u64,
}

impl Snapshot {
    /// Mémoire totale des groupes affichés.
    pub fn listed_bytes(&self) -> u64 {
        self.groups.iter().map(|g| g.bytes).sum()
    }
}

/// Construit la photographie courante.
///
/// `include_system` conserve les processus Apple et les démons du système.
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

/// Application à laquelle rattacher un processus.
struct Anchor {
    name: String,
    bundle: Option<PathBuf>,
    system: bool,
}

/// Rattache un processus à son application mère.
///
/// On remonte la chaîne des parents tant que le processus n'appartient pas
/// lui-même à un bundle : `rust-analyzer` se retrouve ainsi sous son éditeur.
/// Un parent système n'absorbe jamais un processus tiers, sinon tout finirait
/// regroupé sous `launchd`.
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

/// Bundle `.app` le plus externe du chemin, et son nom.
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

/// Emplacements appartenant à macOS ou à Apple.
const SYSTEM_PREFIXES: &[&str] = &[
    "/System/",
    "/usr/",
    "/bin/",
    "/sbin/",
    "/Library/Apple/",
    "/Library/CoreMediaIO/",
    "/Library/PrivilegedHelperTools/com.apple.",
];

/// Un processus est « système » s'il vient de macOS lui-même.
fn is_system(path: &str) -> bool {
    !path.starts_with('/')
        || path.contains("/Cryptexes/")
        || SYSTEM_PREFIXES
            .iter()
            .any(|prefix| path.starts_with(prefix))
}

/// Liste les processus avec leur empreinte mémoire.
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

/// Découpe `count` champs en tête de ligne, et renvoie le reste tel quel.
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

/// Empreinte mémoire par PID, telle que l'affiche le Moniteur d'activité.
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

/// Analyse une taille affichée par `top` : `6257K`, `726M`, `1.2G`, `512B`.
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

/// Pourcentage de mémoire libre d'après `memory_pressure`.
fn free_percent() -> Option<u8> {
    let output = cmd::run("memory_pressure", &["-Q"]).ok()?;
    output
        .stdout
        .lines()
        .find_map(|line| line.split("free percentage:").nth(1))
        .and_then(|value| value.trim().trim_end_matches('%').parse().ok())
}

/// Groupes lancés automatiquement, avec le label de l'agent responsable.
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

/// Lignes de commande complètes, par PID.
///
/// Séparé de la liste principale : seul `inspect` en a besoin.
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

/// Complète un groupe avec la ligne de commande de chacun de ses processus.
pub fn with_command_lines(group: &mut Group) {
    let lines = command_lines();
    for process in &mut group.processes {
        process.args = lines.get(&process.pid).cloned();
    }
}

/// Arbre parent → enfants à l'intérieur d'un groupe.
///
/// Les racines sont les processus dont le parent est hors du groupe.
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

/// Exécutable lancé par un agent, lu depuis son fichier `.plist`.
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

// ── désignation d'un groupe ─────────────────────────────────────────────────

/// Résultat d'une recherche de groupe par nom.
pub enum Lookup<'a> {
    /// Un seul groupe correspond.
    One(&'a Group),
    /// Plusieurs correspondances : il faut préciser.
    Ambiguous(Vec<&'a str>),
    /// Aucune correspondance.
    None,
}

/// Critère de correspondance entre un nom de groupe et une requête.
type Matcher = fn(&str, &str) -> bool;

/// Du plus strict au plus permissif.
///
/// La casse compte au premier tour : c'est ce qui distingue l'application
/// `Claude` de l'exécutable `claude`, et rend les numéros de `ram` fiables.
const MATCHERS: [Matcher; 4] = [
    |name, query| name == query,
    |name, query| name.to_lowercase() == query.to_lowercase(),
    |name, query| name.to_lowercase().contains(&query.to_lowercase()),
    matches_words,
];

/// Retrouve un groupe à partir d'un nom approximatif.
///
/// Par ordre de priorité : nom exact, nom contenant la requête, puis
/// correspondance mot à mot (`vs code` retrouve `Visual Studio Code`).
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

/// `vs code` et `vscode` retrouvent `Visual Studio Code`.
///
/// Les mots du nom consomment la requête lettre à lettre, dans l'ordre : chaque
/// mot en avale le plus long préfixe commun, et peut n'en avaler aucun.
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

// ── numéros mémorisés entre deux commandes ──────────────────────────────────

/// Fichier where `ram` note la liste affichée, pour que `kill 3` fonctionne.
fn state_path() -> PathBuf {
    std::env::temp_dir().join("detox-mac-ram.json")
}

/// Mémorise les groupes affichés, dans l'ordre.
pub fn remember(groups: &[&Group]) {
    let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    if let Ok(json) = serde_json::to_string(&names) {
        let _ = std::fs::write(state_path(), json);
    }
}

/// Nom du groupe affiché à cette position lors du dernier `detox-mac ram`.
pub fn recall(index: usize) -> Option<String> {
    let raw = std::fs::read_to_string(state_path()).ok()?;
    let names: Vec<String> = serde_json::from_str(&raw).ok()?;
    index
        .checked_sub(1)
        .and_then(|position| names.get(position))
        .cloned()
}

// ── arrêt d'une application ─────────────────────────────────────────────────

/// Compte rendu d'un arrêt de groupe.
#[derive(Debug, Clone, Serialize)]
pub struct Killed {
    pub group: String,
    pub signal: &'static str,
    /// Mémoire occupée par les processus visés.
    pub bytes: u64,
    /// Processus effectivement arrêtés.
    pub terminated: Vec<u32>,
    /// Processus toujours vivants après le signal.
    pub survived: Vec<u32>,
    pub errors: Vec<String>,
}

/// Envoie un signal à tous les processus d'un groupe.
///
/// Les processus racines partent en premier : les helpers d'un Electron
/// s'arrêtent d'eux-mêmes et n'affichent pas de fenêtre de plantage.
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
            // Un enfant déjà parti avec son parent n'est pas une erreur.
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

/// Vrai si le processus existe encore.
fn is_alive(pid: u32) -> bool {
    cmd::run("kill", &["-0", &pid.to_string()]).is_ok()
}

/// Ancêtres du processus courant, pour éviter de se saborder sans le savoir.
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
