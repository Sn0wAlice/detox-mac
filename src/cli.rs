//! Définition de la ligne de commande.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::format;
use crate::task::agents::Scope;
use crate::task::clean::Target;
use crate::ui::ColorChoice;

/// Outil de maintenance macOS : nettoyage, diagnostic et agents de démarrage.
#[derive(Debug, Parser)]
#[command(
    name = "detox-mac",
    version,
    about,
    long_about = None,
    propagate_version = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(flatten)]
    pub options: Options,

    #[command(subcommand)]
    pub command: Command,
}

/// Options valables pour toutes les sous-commandes.
#[derive(Debug, Args)]
pub struct Options {
    /// Ne rien modifier : mesure et affiche ce qui serait fait.
    #[arg(short = 'n', long, global = true)]
    pub dry_run: bool,

    /// Répondre oui à toutes les confirmations.
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// N'afficher que les avertissements et les erreurs.
    #[arg(short, long, global = true, conflicts_with = "json")]
    pub quiet: bool,

    /// Sortie JSON, pour les scripts.
    #[arg(long, global = true)]
    pub json: bool,

    /// Coloration de la sortie.
    #[arg(long, global = true, value_name = "QUAND", default_value = "auto")]
    pub color: ColorChoice,
}

/// Cible acceptée sur la ligne de commande : une zone précise, ou `all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum TargetArg {
    /// Toutes les cibles, sauf les simulateurs iOS (à demander explicitement).
    All,
    /// Caches utilisateur (`~/Library/Caches`).
    Cache,
    /// Corbeille utilisateur (`~/.Trash`).
    Trash,
    /// Corbeille utilisateur + corbeilles des volumes montés.
    TrashAll,
    /// Journaux utilisateur (`~/Library/Logs`).
    Logs,
    /// Fichiers `.DS_Store` du dossier personnel.
    DsStore,
    /// Cache de téléchargement Homebrew.
    Homebrew,
    /// Docker : conteneurs, images et caches de build inutilisés.
    Docker,
    /// Données Xcode : DerivedData, DeviceSupport, caches du simulateur.
    Xcode,
    /// Appareils du simulateur iOS.
    Simulators,
}

impl TargetArg {
    /// Cible concrète, ou `None` pour `all`.
    fn target(self) -> Option<Target> {
        match self {
            TargetArg::All => None,
            TargetArg::Cache => Some(Target::Cache),
            TargetArg::Trash => Some(Target::Trash),
            TargetArg::TrashAll => Some(Target::TrashAll),
            TargetArg::Logs => Some(Target::Logs),
            TargetArg::DsStore => Some(Target::DsStore),
            TargetArg::Homebrew => Some(Target::Homebrew),
            TargetArg::Docker => Some(Target::Docker),
            TargetArg::Xcode => Some(Target::Xcode),
            TargetArg::Simulators => Some(Target::Simulators),
        }
    }

    /// Développe les arguments en cibles concrètes, sans doublon et dans l'ordre.
    pub fn expand(args: &[TargetArg]) -> Vec<Target> {
        fn push(targets: &mut Vec<Target>, target: Target) {
            if !targets.contains(&target) {
                targets.push(target);
            }
        }

        let mut targets = Vec::new();
        for arg in args {
            match arg.target() {
                Some(target) => push(&mut targets, target),
                None => Target::DEFAULT
                    .iter()
                    .for_each(|&target| push(&mut targets, target)),
            }
        }

        targets
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Résumé de la machine et de l'espace récupérable.
    Info,

    /// Mesurer l'espace récupérable, sans rien supprimer.
    Scan {
        /// Cibles à mesurer ; `all` pour tout mesurer (défaut : les plus rapides).
        #[arg(value_name = "CIBLE")]
        targets: Vec<TargetArg>,
    },

    /// Nettoyer une ou plusieurs cibles ; `all` pour tout nettoyer d'un coup.
    Clean {
        /// Cibles à nettoyer, ou `all`.
        #[arg(value_name = "CIBLE", required = true)]
        targets: Vec<TargetArg>,
    },

    /// Lister les applications installées par taille.
    Apps {
        /// Nombre d'applications affichées (0 = toutes).
        #[arg(short, long, value_name = "N", default_value_t = 15)]
        top: usize,
    },

    /// Rechercher les fichiers volumineux.
    Files {
        /// Taille minimale (ex. 500M, 1.5G).
        #[arg(short, long, value_name = "TAILLE", default_value = "500M", value_parser = format::parse_size)]
        min: u64,

        /// Nombre de fichiers affichés (0 = tous).
        #[arg(short, long, value_name = "N", default_value_t = 20)]
        top: usize,

        /// Dossier de départ (par défaut : le dossier personnel).
        #[arg(short, long, value_name = "CHEMIN")]
        path: Option<PathBuf>,

        /// Profondeur maximale de parcours.
        #[arg(short, long, value_name = "N", default_value_t = 8)]
        depth: usize,
    },

    /// Inspecter la mémoire vive, regroupée par application.
    Ram {
        /// Nombre de groupes affichés (0 = tous).
        #[arg(short, long, value_name = "N", default_value_t = 15)]
        top: usize,

        /// Inclure les processus de macOS et des applications Apple.
        #[arg(short, long)]
        all: bool,

        /// Détailler les processus de chaque groupe.
        #[arg(short, long)]
        detail: bool,

        /// Masquer les groupes en dessous de cette taille.
        #[arg(short, long, value_name = "TAILLE", default_value = "0", value_parser = format::parse_size)]
        min: u64,
    },

    /// Arrêter tous les processus d'une application.
    Kill {
        /// Nom de l'application, ou numéro affiché par `detox-mac ram`.
        #[arg(value_name = "CIBLE", required = true, num_args = 1..)]
        target: Vec<String>,

        /// Envoyer SIGKILL au lieu de SIGTERM (arrêt brutal, sans sauvegarde).
        #[arg(short, long)]
        force: bool,

        /// Autoriser à viser un composant de macOS ou une application Apple.
        #[arg(long)]
        system: bool,
    },

    /// Gérer les agents et démons de démarrage.
    Agents {
        #[command(subcommand)]
        command: AgentCommand,
    },

    /// Opérations système ponctuelles.
    Sys {
        #[command(subcommand)]
        command: SysCommand,
    },

    /// Générer la complétion pour un shell.
    Completions {
        /// Shell cible.
        #[arg(value_name = "SHELL")]
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Lister les agents installés.
    List {
        /// N'afficher que les agents tiers.
        #[arg(short, long)]
        third_party: bool,

        /// Restreindre à un emplacement.
        #[arg(short, long, value_name = "EMPLACEMENT")]
        scope: Option<Scope>,
    },

    /// Désactiver des agents tiers (`launchctl unload`, sans suppression).
    Disable {
        #[command(flatten)]
        selection: Selection,
    },

    /// Réactiver des agents tiers (`launchctl load`).
    Enable {
        #[command(flatten)]
        selection: Selection,
    },

    /// Supprimer définitivement des agents tiers.
    Remove {
        #[command(flatten)]
        selection: Selection,
    },
}

/// Sélection d'agents : labels explicites ou tous les agents tiers.
#[derive(Debug, Args)]
pub struct Selection {
    /// Labels à traiter (ex. com.docker.helper).
    #[arg(value_name = "LABEL", required_unless_present = "all_third_party")]
    pub labels: Vec<String>,

    /// Tous les agents tiers. Les agents Apple ne sont jamais touchés.
    #[arg(long, conflicts_with = "labels")]
    pub all_third_party: bool,

    /// Restreindre à un emplacement.
    #[arg(short, long, value_name = "EMPLACEMENT")]
    pub scope: Option<Scope>,
}

#[derive(Debug, Subcommand)]
pub enum SysCommand {
    /// Vider le cache DNS (sudo).
    Dns,
    /// Réinitialiser l'index Spotlight (sudo).
    Spotlight,
    /// Libérer la mémoire inactive (sudo).
    Memory,
    /// Purger les instantanés Time Machine locaux.
    Snapshots,
    /// Lister les mises à jour macOS disponibles.
    Updates,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn all_expands_to_every_default_target() {
        assert_eq!(
            TargetArg::expand(&[TargetArg::All]),
            Target::DEFAULT.to_vec()
        );
    }

    #[test]
    fn expansion_keeps_order_and_drops_duplicates() {
        let targets = TargetArg::expand(&[TargetArg::Logs, TargetArg::All, TargetArg::Simulators]);
        assert_eq!(targets[0], Target::Logs);
        assert_eq!(targets.last(), Some(&Target::Simulators));
        assert_eq!(targets.iter().filter(|t| **t == Target::Logs).count(), 1);
    }
}
