# detox-mac

![](./.github/banner.png)

> macOS maintenance from the command line, written in Rust. 🦀
> Like *CleanMyMac*, but open-source, scriptable, and no BS.

```bash
detox-mac info          # what the machine holds, and what can be reclaimed
detox-mac ram           # what is eating memory, grouped by application
detox-mac files dev     # build residue sleeping in your projects
detox-mac clean all -n  # what would be deleted, touching nothing
detox-mac clean all     # go
```

---

## ✨ Commands

| Command | Purpose |
|---|---|
| `info` | Machine summary (macOS, CPU, RAM, swap, disk, uptime), reclaimable space, startup agents |
| `scan [TARGET…]` | Measure reclaimable space, **never deleting anything** |
| `clean <TARGET…>` | Clean one or more targets (`all` for everything) |
| `apps` | Installed applications, largest first |
| `files large` | Large files under a directory |
| `files dev` | Build residue of local projects (`target`, `node_modules`…) |
| `files clean` | Remove that residue, past a given age |
| `ram` | Processes grouped by parent application, sorted by memory |
| `inspect <target>` | Process tree of one application, with CPU, age and arguments |
| `kill <target>` | Stop every process of one application (by name or number) |
| `agents list\|disable\|enable\|remove` | Startup agents and daemons (`launchd`) |
| `sys dns\|spotlight\|memory\|snapshots\|updates` | One-off system operations |
| `completions <shell>` | bash / zsh / fish / elvish / powershell completions |

### Cleaning targets

| Target | Contents |
|---|---|
| `cache` | `~/Library/Caches` |
| `trash` | `~/.Trash` |
| `trash-all` | `~/.Trash` plus the `.Trashes` of every mounted volume |
| `logs` | `~/Library/Logs` |
| `ds-store` | `.DS_Store` files in the home directory |
| `homebrew` | Homebrew download cache (`brew cleanup --prune=all -s`) |
| `docker` | Unused containers, images and build caches — **never volumes** |
| `xcode` | DerivedData, DeviceSupport (iOS/watchOS/tvOS), simulator caches |
| `simulators` | iOS simulator devices — **left out of `all`**, ask for it explicitly |
| `all` | Every target above except `simulators` |

Docker is only cleaned when it is installed **and** the daemon answers;
otherwise the target is skipped with the reason. Same for Homebrew and Xcode.

---

## 🚀 Install

```bash
git clone https://github.com/Sn0wAlice/detox-mac.git
cd detox-mac
cargo install --path .
```

Or, without `cargo install`:

```bash
cargo build --release
sudo cp target/release/detox-mac /usr/local/bin/
```

Requires macOS and Rust 1.85+ (2024 edition).

---

## 🔧 Usage

### Global options

| Option | Effect |
|---|---|
| `-n`, `--dry-run` | Measure and print what would happen, change nothing |
| `-y`, `--yes` | Answer yes to every confirmation (scripts, cron) |
| `-q`, `--quiet` | Print warnings and errors only |
| `--json` | JSON on stdout, for scripts |
| `--color <auto\|always\|never>` | Colour (also honours `NO_COLOR`) |

### Examples

```bash
# Quick diagnosis
detox-mac info

# Full measurement, slow targets included (.DS_Store, Docker…)
detox-mac scan all

# Clean only the harmless things, no confirmation
detox-mac clean cache logs trash --yes

# See what a full cleanup would free
detox-mac clean all --dry-run

# The 30 largest applications
detox-mac apps --top 30

# Files over 2 GB in a specific directory
detox-mac files large --min 2G --path ~/Movies

# Build residue untouched for more than a week
detox-mac files dev
detox-mac files clean --older-than 30

# What is holding memory, then stop it
detox-mac ram --detail --top 5
detox-mac inspect discord
detox-mac kill discord

# Third-party startup agents, then disable one
detox-mac agents list --third-party
detox-mac agents disable com.docker.helper

# Purgeable space and updates
detox-mac sys snapshots
detox-mac sys updates
```

### Progress

Long operations draw a progress bar or a spinner **on stderr**, so stdout stays
pipeable and `--json` stays parseable:

```
Measuring ████████████░░░░░░░░░░░░ 12/24  ~/Documents/mlab.other/nav-ext
Looking for projects ⠹ 3481  ~/Downloads/card
```

It is disabled with `--quiet`, with `--json`, and whenever stderr is not a
terminal. `--color always` forces it back on.

### Shell completions

```bash
detox-mac completions zsh > ~/.zsh/completions/_detox-mac
```

### JSON output

```bash
detox-mac scan all --json | jq '.total'
detox-mac files dev --json | jq '[.residue[].bytes] | add'
detox-mac ram --json | jq '.groups[] | select(.autostart) | .name'
detox-mac inspect spotify --json | jq '.group.processes[].args'
detox-mac agents list --json | jq '.agents[] | select(.apple == false) | .label'
```

---

## 🧹 Build residue

`detox-mac files dev` looks for directories the toolchain knows how to rebuild
— `target`, `node_modules`, `.gradle`, `vendor`… — and only reports those that
have not moved for a while.

```bash
detox-mac files dev                          # untouched for 7 days
detox-mac files dev --older-than 30          # only the really dormant ones
detox-mac files dev --lang rust node         # one ecosystem in particular
detox-mac files clean --older-than 30        # remove them, after confirmation
detox-mac files clean --path ~/dev -n        # dry run on a specific directory
```

```
Build residue in ~ — untouched for 10 day(s) (58 directories, 106.40 GB)
    26.17 GB  target           ~/Documents/mlab.sh/vuln.mlab.sh         rust · 12d
    16.35 GB  target           ~/Documents/mlab.sh/news.mlab.sh         rust · 11d
     8.48 GB  target           ~/Documents/mlab.other/mlab-cloudflare   rust · 12d
```

### How a directory is recognised

**Never by its name alone.** A `target` is only picked up when it sits next to a
`Cargo.toml` (or a `pom.xml`), a `build` only when the matching project file is
there, and a CMake `build` only when it actually contains a `CMakeCache.txt`. A
lone `target` with no manifest is left alone.

| Language | Marker | Directories | `--native` command |
|---|---|---|---|
| `rust` | `Cargo.toml` | `target` | `cargo clean --offline` |
| `node` | `package.json` | `node_modules`, `.next`, `.nuxt`, `.turbo`, `.parcel-cache`, `.svelte-kit`, `.angular`, `.astro` | — |
| `python` | `pyproject.toml`, `setup.py`, `requirements.txt`, `Pipfile` | `__pycache__`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.tox` | — |
| `gradle` | `build.gradle(.kts)`, `settings.gradle` | `build`, `.gradle` | `./gradlew clean --offline` |
| `maven` | `pom.xml` | `target` | `mvn -o -q clean` |
| `swift` | `Package.swift` | `.build` | `swift package clean` |
| `dotnet` | `*.csproj`, `*.fsproj`, `*.vbproj`, `*.sln` | `bin`, `obj` | `dotnet clean` |
| `php` | `composer.json` | `vendor` | — |
| `elixir` | `mix.exs` | `_build`, `deps` | `mix clean` |
| `dart` | `pubspec.yaml` | `build`, `.dart_tool` | `flutter clean` |
| `zig` | `build.zig` | `zig-cache`, `.zig-cache`, `zig-out` | — |
| `go` | `go.mod` | `vendor` | — |
| `cmake` | `CMakeLists.txt` + `CMakeCache.txt` | `build`, `cmake-build-debug`, `cmake-build-release` | — |

- **Age** is the time since the directory *or its top-level content* last
  changed: a `target` you compiled into this morning is never offered, however
  old the project is.
- Python virtualenvs (`.venv`), Docker volumes and hand-written `dist`
  directories are **not** touched: too ambiguous.
- The walk never descends into a directory already identified as regenerable,
  nor into hidden directories, `Library`, `Applications`, `Pictures`, `Movies`.

### Removal, or the language's own tool?

By default, `files clean` **removes the directory**. For `cargo` as for `npm`
the result is identical: `cargo clean` wipes `target/` entirely, and `npm` has
no clean command — removing `node_modules` *is* the official method, `npm ci`
puts it back.

`--native` hands over to the language's tool where one exists:

```bash
detox-mac files clean --native --older-than 30
```

- The tool is only used when it is **installed** (`gradlew` is looked up in the
  project, everything else in `PATH`); otherwise the directory is removed, with
  no failure and no question.
- Commands run **offline** (`--offline`, `-o`) and non-interactively: a cleanup
  must never trigger a dependency download.
- When the tool leaves things behind — `mix clean` does not touch `deps/`,
  `dotnet clean` only removes what MSBuild tracked — the directory is removed
  afterwards and the line says `then removed the leftovers`.
- It is slower: a `gradlew clean` boots a JVM. Hence plain removal by default.

---

## 🧠 Memory

`detox-mac ram` does what Activity Monitor does, more readably: it groups
processes **by parent application**, so an Electron app with fifteen helpers is
a single line.

```bash
detox-mac ram                      # top 15 applications outside macOS
detox-mac ram --detail             # with the processes of each group
detox-mac ram --all                # include macOS and Apple applications
detox-mac ram --min 100M --top 0   # everything above 100 MB
```

```
Memory
  Physical         16.00 GB — 9.64 GB used, 358.6 MB free, 5.37 GB inactive
  Swap             2.44 GB used of 4.00 GB
  Pressure         71% of memory free

Processes outside macOS (21 group(s) — 7.36 GB)
    1.    1.64 GB  Visual Studio Code                 14 proc.
    2.    1.60 GB  Claude                             27 proc.
    3.   796.0 MB  Spotify                            6 proc.
    4.    92.0 MB  Little Snitch                      starts at login · 1 proc.
    5.    19.0 MB  WiFiman Desktop                    starts at login · 1 proc.
```

- A process with no bundle (`node`, `rust-analyzer`, `zsh`…) is attached to the
  application that launched it — but never to a system parent, otherwise
  everything would end up under `launchd`.
- **`starts at login`** marks an application launched by a `launchd` agent:
  that is where updaters and background daemons hide. Stop one with
  `detox-mac agents disable <label>`.
- The figure shown is the real footprint (`top`), the one Activity Monitor
  reports; `ps` is the fallback when it is unavailable.

### Understanding one application

```bash
detox-mac inspect 2            # the 2nd group of the last detox-mac ram
detox-mac inspect spotify      # by name
detox-mac inspect claude       # `claude` (CLI) and `Claude` (app) stay distinct
detox-mac inspect spotify -s   # without command lines
```

```
Spotify
  Bundle           /Applications/Spotify.app
  Memory           855.0 MB — 6 processes
  CPU              0.7% (average since launch)

    266.0 MB  Spotify [939]                                    0.6% · 3d 0h
    ├─   339.0 MB  Spotify Helper (Renderer) [1288]            0.0% · 3d 0h
    │              --type=renderer --user-data-dir=/Users/…
    └─    20.0 MB  Spotify Helper [1248]                       0.0% · 3d 0h
                   --type=utility --utility-sub-type=storage.mojom.StorageService
```

The tree follows the real parent/child chain, and the arguments
(`--type=renderer`, `--type=gpu-process`…) say what each helper is for. When
the application is started by a `launchd` agent, its label is printed together
with the command that disables it.

The `%` is the average CPU since the process started (what `ps` reports), not an
instant sample: that is what exposes a background process grinding away.

### Stopping a whole application

The numbers in the left column are shortcuts for `kill`:

```bash
detox-mac kill 3               # the 3rd group of the last detox-mac ram
detox-mac kill discord         # by name
detox-mac kill vs code         # `vs code` finds `Visual Studio Code`
detox-mac kill spotify --force # SIGKILL, when SIGTERM was not enough
detox-mac kill -n discord      # dry run: lists the targeted processes
```

`kill` sends `SIGTERM` to **every** process of the group, roots first so that
helpers shut down cleanly, then checks who survived.

- A number is only a shortcut to a **name**: the target is resolved again and
  printed before the confirmation, so a ranking that moved between two commands
  can never kill the wrong application.
- An ambiguous name is never guessed: the candidates are listed.
- macOS components are refused without `--system`.
- `detox-mac` never kills itself; when the target holds the terminal running the
  command, it says so before asking.

---

## 🛡️ Safety

`detox-mac` deletes files. It is built so that never happens by surprise.

- **Confirmation** before any destructive operation (unless `--yes` or
  `--dry-run`). Outside a terminal, it refuses to act without `--yes`.
- **Dry run** (`-n`): exact measurement of what would go, zero modification.
- **Apple agents are never disabled nor removed**, even with `--all-third-party`.
- **Docker volumes are never pruned**: they hold your data.
- **iOS simulators are excluded from `all`**: several GB to download again.
- Symlinks are never followed, when walking or when deleting.

### Sudo

`sys dns`, `sys spotlight`, `sys memory`, and the agents under `/Library`,
need root. Without it the operation is **skipped** with a clear message — never
half attempted:

```bash
sudo detox-mac sys dns
```

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success (a skipped operation is still a success) |
| `1` | Something failed, or a confirmation was declined |
| `2` | Usage error (invalid arguments) |

---

## 🏗️ Architecture

```
src/
├── main.rs            # Entry point, exit code
├── cli.rs             # Command-line definition (clap)
├── app.rs             # Command dispatch and rendering (text / JSON)
├── format.rs          # Human-readable sizes, durations, paths
├── ui.rs              # Colours, progress bars, confirmations
├── sys/               # System access
│   ├── cmd.rs         #   Running external commands
│   ├── fsx.rs         #   Walking, measuring and removing files
│   └── machine.rs     #   Machine info (sw_vers, sysctl, vm_stat, df)
└── task/              # Business logic, independent of display
    ├── clean.rs       #   Cleaning targets: measure and remove
    ├── dev.rs         #   Build residue, per ecosystem
    ├── docker.rs      #   Docker detection and prunes
    ├── agents.rs      #   launchd startup agents
    ├── maintenance.rs #   DNS, Spotlight, memory, snapshots, updates
    ├── ram.rs         #   Memory snapshot, grouping, inspection, shutdown
    └── scan.rs        #   Applications and large files
```

Tasks return serialisable structures; `app.rs` alone decides how they look.
That is what makes `--json` work on every command without duplicating a single
line of logic, and what lets the slow loops report progress.

**Dependencies**: `clap`, `clap_complete`, `serde`, `serde_json`.

```bash
cargo test          # unit tests
cargo clippy        # lints
cargo fmt           # formatting
```

---

## ❤️ Why?

Because we love our Macs, but not bloated cleanup apps.

## 📄 License

GPL-3.0-or-later — see [LICENSE](./LICENSE).
