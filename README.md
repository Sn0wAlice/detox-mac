# detox-mac

![](./.github/banner.png)

> Outil de maintenance macOS en ligne de commande, écrit en Rust. 🦀
> Comme *CleanMyMac*, mais open-source, scriptable, et sans bullshit.

```bash
detox-mac info          # ce que contient la machine, et ce qui est récupérable
detox-mac ram           # qui mange la RAM, groupé par application
detox-mac kill discord  # et on coupe tout Discord d'un coup
detox-mac clean all -n  # ce qui serait supprimé, sans rien toucher
detox-mac clean all     # on y va
```

---

## ✨ Ce que ça fait

| Commande | Rôle |
|---|---|
| `info` | Résumé machine (macOS, CPU, RAM, disque, uptime) + espace récupérable + agents de démarrage |
| `scan [CIBLE…]` | Mesure l'espace récupérable, **sans jamais rien supprimer** |
| `clean <CIBLE…>` | Nettoie une ou plusieurs cibles (`all` pour tout) |
| `ram` | Processus regroupés par application mère, triés par mémoire |
| `kill <cible>` | Arrête tous les processus d'une application (nom ou numéro) |
| `apps` | Applications installées, de la plus lourde à la plus légère |
| `files` | Fichiers volumineux du dossier personnel |
| `agents list\|disable\|enable\|remove` | Agents et démons de démarrage (`launchd`) |
| `sys dns\|spotlight\|memory\|snapshots\|updates` | Opérations système ponctuelles |
| `completions <shell>` | Complétion bash / zsh / fish / elvish / powershell |

### Cibles de nettoyage

| Cible | Contenu |
|---|---|
| `cache` | `~/Library/Caches` |
| `trash` | `~/.Trash` |
| `trash-all` | `~/.Trash` + `.Trashes` de chaque volume monté |
| `logs` | `~/Library/Logs` |
| `ds-store` | Fichiers `.DS_Store` du dossier personnel |
| `homebrew` | Cache de téléchargement Homebrew (`brew cleanup --prune=all -s`) |
| `docker` | Conteneurs, images et caches de build inutilisés — **jamais les volumes** |
| `xcode` | DerivedData, DeviceSupport (iOS/watchOS/tvOS), caches du simulateur |
| `simulators` | Appareils du simulateur iOS — **exclu de `all`**, à demander explicitement |
| `all` | Toutes les cibles ci-dessus sauf `simulators` |

Docker n'est nettoyé que s'il est installé **et** que le démon répond ; sinon la
cible est simplement ignorée. Idem pour Homebrew et Xcode.

---

## 🧠 Inspecter la mémoire

`detox-mac ram` fait ce que fait le Moniteur d'activité, en plus lisible : il
regroupe les processus **par application mère**, pour qu'un Electron avec ses
quinze helpers compte pour une seule ligne.

```bash
detox-mac ram                      # top 15 des applications hors macOS
detox-mac ram --detail             # avec le détail des processus
detox-mac ram --all                # inclut macOS et les applications Apple
detox-mac ram --min 100M --top 0   # tout ce qui dépasse 100 Mo
```

```
Mémoire
  Physique         16.00 Go — 9.64 Go utilisée, 358.6 Mo libre, 5.37 Go inactive
  Swap             2.44 Go utilisé sur 4.00 Go
  Pression         71 % de mémoire libre

Processus hors macOS (21 groupe(s) — 7.36 Go)
    1.    1.64 Go  Visual Studio Code                 14 proc.
    2.    1.60 Go  Claude                             27 proc.
    3.   796.0 Mo  Spotify                            6 proc.
    4.    92.0 Mo  Little Snitch                      démarrage auto · 1 proc.
    5.    19.0 Mo  WiFiman Desktop                    démarrage auto · 1 proc.
```

- Un processus sans bundle (`node`, `rust-analyzer`, `zsh`…) est rattaché à
  l'application qui l'a lancé — mais jamais à un parent système, sinon tout
  finirait sous `launchd`.
- **`démarrage auto`** signale une application lancée par un agent `launchd` :
  c'est là que se cachent les mises à jour, agents et daemons qui tournent sans
  qu'on le leur ait demandé. On les coupe avec `detox-mac agents disable <label>`.
- La mémoire affichée est l'empreinte réelle (`top`), celle du Moniteur
  d'activité ; `ps` sert de repli si elle n'est pas disponible.

### Arrêter une application entière

Les numéros de la colonne de gauche servent de raccourci pour `kill` :

```bash
detox-mac kill 3               # le 3e groupe du dernier detox-mac ram
detox-mac kill discord         # par nom
detox-mac kill vs code         # « vs code » retrouve « Visual Studio Code »
detox-mac kill spotify --force # SIGKILL, si SIGTERM n'a pas suffi
detox-mac kill -n discord      # simulation : liste les processus visés
```

`kill` envoie `SIGTERM` à **tous** les processus du groupe, les racines d'abord
pour que les helpers s'arrêtent proprement, puis vérifie qui a survécu.

- Le numéro n'est qu'un raccourci vers un **nom** : la cible est re-résolue et
  affichée avant la confirmation, donc un classement qui a bougé entre deux
  commandes ne peut pas faire tuer la mauvaise application.
- Un nom ambigu n'est jamais deviné : les candidats sont listés.
- Les composants de macOS sont refusés sans `--system`.
- Le processus `detox-mac` lui-même n'est jamais tué ; si la cible contient le
  terminal qui exécute la commande, l'outil le signale avant de demander confirmation.

---

## 🚀 Installation

```bash
git clone https://github.com/Sn0wAlice/detox-mac.git
cd detox-mac
cargo install --path .
```

Ou, sans `cargo install` :

```bash
cargo build --release
sudo cp target/release/detox-mac /usr/local/bin/
```

Prérequis : macOS et Rust 1.85+ (édition 2024).

---

## 🔧 Utilisation

### Options globales

| Option | Effet |
|---|---|
| `-n`, `--dry-run` | Mesure et affiche ce qui serait fait, sans rien modifier |
| `-y`, `--yes` | Répond oui à toutes les confirmations (scripts, cron) |
| `-q`, `--quiet` | N'affiche que les avertissements et les erreurs |
| `--json` | Sortie JSON sur stdout, pour les scripts |
| `--color <auto\|always\|never>` | Coloration (respecte aussi `NO_COLOR`) |

### Exemples

```bash
# Diagnostic rapide
detox-mac info

# Mesure complète, y compris les cibles lentes (.DS_Store, Docker…)
detox-mac scan all

# Nettoyer seulement ce qui est sans risque, sans confirmation
detox-mac clean cache logs trash --yes

# Voir ce qu'un nettoyage complet libérerait
detox-mac clean all --dry-run

# Ce qui occupe la RAM, avec le détail des processus
detox-mac ram --detail --top 5

# Couper une application qui traîne
detox-mac kill discord

# Les 30 plus grosses applications
detox-mac apps --top 30

# Les fichiers de plus de 2 Go dans un dossier précis
detox-mac files --min 2G --path ~/Movies

# Agents de démarrage tiers, puis désactivation de l'un d'eux
detox-mac agents list --third-party
detox-mac agents disable com.docker.helper

# Espace purgeable et mises à jour
detox-mac sys snapshots
detox-mac sys updates
```

### Complétion shell

```bash
detox-mac completions zsh > ~/.zsh/completions/_detox-mac
```

### Sortie JSON

```bash
detox-mac ram --json | jq '.groups[] | select(.autostart) | .name'
detox-mac scan all --json | jq '.total'
detox-mac agents list --json | jq '.agents[] | select(.apple == false) | .label'
```

---

## 🛡️ Sécurité

`detox-mac` supprime des fichiers : l'outil est construit pour que ça n'arrive jamais par surprise.

- **Confirmation** avant toute opération destructive (sauf `--yes` ou `--dry-run`).
  Hors terminal (script, pipe), l'outil refuse d'agir sans `--yes`.
- **Mode simulation** (`-n`) : mesure exacte de ce qui serait supprimé, zéro modification.
- **Les agents Apple ne sont jamais désactivés ni supprimés**, même avec `--all-third-party`.
- **Les volumes Docker ne sont jamais purgés** : ils contiennent vos données.
- **Les simulateurs iOS sont exclus de `all`** : plusieurs Go à retélécharger.
- Les liens symboliques ne sont jamais suivis lors des parcours ni des suppressions.

### Sudo

`sys dns`, `sys spotlight`, `sys memory`, ainsi que les agents de `/Library`,
nécessitent les privilèges root. Sans eux, l'opération est **ignorée** avec un
message clair — jamais tentée à moitié :

```bash
sudo detox-mac sys dns
```

### Codes de sortie

| Code | Signification |
|---|---|
| `0` | Succès (une opération ignorée reste un succès) |
| `1` | Au moins une opération a échoué, ou confirmation refusée |
| `2` | Erreur d'usage (arguments invalides) |

---

## 🏗️ Architecture

```
src/
├── main.rs            # Point d'entrée, code de sortie
├── cli.rs             # Définition de la ligne de commande (clap)
├── app.rs             # Dispatch des commandes et mise en forme (texte / JSON)
├── format.rs          # Tailles lisibles, chemins abrégés
├── ui.rs              # Couleurs, confirmations, sortie terminal
├── sys/               # Accès système
│   ├── cmd.rs         #   Exécution de commandes externes
│   ├── fsx.rs         #   Parcours, mesure et suppression de fichiers
│   └── machine.rs     #   Infos machine (sw_vers, sysctl, vm_stat, df)
└── task/              # Logique métier, indépendante de l'affichage
    ├── clean.rs       #   Cibles de nettoyage : mesure et suppression
    ├── docker.rs      #   Détection Docker et purges
    ├── agents.rs      #   Agents de démarrage launchd
    ├── ram.rs         #   Photographie mémoire, regroupement, arrêt de groupe
    ├── maintenance.rs #   DNS, Spotlight, mémoire, instantanés, mises à jour
    └── scan.rs        #   Applications et gros fichiers
```

Les tâches renvoient des structures sérialisables ; `app.rs` décide seul de
l'affichage. C'est ce qui permet d'avoir `--json` sans dupliquer une ligne de logique.

**Dépendances** : `clap`, `clap_complete`, `serde`, `serde_json`.

```bash
cargo test          # tests unitaires
cargo clippy        # lint
cargo fmt           # format
```

---

## 🔄 Migration depuis la 0.1 (TUI)

La 0.2 remplace l'interface TUI par de vraies sous-commandes : scriptable,
composable, testable.

| Avant (TUI) | Maintenant |
|---|---|
| Onglet *Général* / *Nettoyage* | `detox-mac clean <cible>` |
| Touche `d` (simulation) | `--dry-run` |
| Touche `Espace` + `r` (exécution groupée) | `detox-mac clean cache logs trash` |
| Onglet *Système* | `detox-mac sys <opération>` |
| Onglet *Démarrage* | `detox-mac agents <sous-commande>` |
| Dashboard de démarrage | `detox-mac info` |
| *(nouveau)* | `detox-mac ram` — inspection mémoire par application |
| *(nouveau)* | `detox-mac kill <nom>` — arrêt d'une application entière |

`ratatui` et `crossterm` ne sont plus des dépendances.

---

## ❤️ Pourquoi ?

Parce qu'on aime nos Macs, mais pas les applis de nettoyage bouffies.

## 📄 Licence

GPL-3.0-or-later — voir [LICENSE](./LICENSE).
