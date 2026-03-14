use crate::modules::{
    cache, dsstore, homebrew, launch, login, logs, scanner, sysinfo, system, trash, utils, xcode,
};

#[derive(Clone, Copy, PartialEq)]
pub enum TaskStatus {
    Pending,
    Done,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TaskAction {
    // Infos
    SystemInfo,
    ListLargeApps,
    FindLargeFiles,
    CheckUpdates,
    // Nettoyage
    CleanCache,
    CleanTrash,
    CleanAllTrashes,
    CleanLogs,
    CleanDsStore,
    CleanHomebrew,
    CleanXcode,
    // Système
    FlushDns,
    FreeSpace,
    ReindexSpotlight,
    PurgeRam,
    // Démarrage
    ListLoginItemsDetailed,
    ListThirdPartyAgents,
    DisableThirdPartyAgents,
    EnableThirdPartyAgents,
    RemoveUserLaunchAgents,
    RemoveAllLaunchItems,
}

impl TaskAction {
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            TaskAction::CleanCache
                | TaskAction::CleanTrash
                | TaskAction::CleanAllTrashes
                | TaskAction::CleanLogs
                | TaskAction::CleanDsStore
                | TaskAction::CleanHomebrew
                | TaskAction::CleanXcode
                | TaskAction::FlushDns
                | TaskAction::FreeSpace
                | TaskAction::ReindexSpotlight
                | TaskAction::PurgeRam
                | TaskAction::DisableThirdPartyAgents
                | TaskAction::RemoveUserLaunchAgents
                | TaskAction::RemoveAllLaunchItems
        )
    }
}

pub struct Task {
    pub name: &'static str,
    pub description: &'static str,
    pub action: TaskAction,
    pub checked: bool,
    pub status: TaskStatus,
    pub needs_root: bool,
}

pub struct Tab {
    pub name: &'static str,
    pub tasks: Vec<Task>,
    pub selected: usize,
}

pub struct App {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub log: Vec<String>,
    pub log_scroll: usize,
    pub dry_run: bool,
    pub confirming: bool,
}

impl Task {
    fn new(
        name: &'static str,
        description: &'static str,
        action: TaskAction,
        needs_root: bool,
    ) -> Self {
        Self {
            name,
            description,
            action,
            checked: false,
            status: TaskStatus::Pending,
            needs_root,
        }
    }
}

impl App {
    pub fn new() -> Self {
        let startup_log = sysinfo::get_system_info();

        let tabs = vec![
            Tab {
                name: " Général ",
                selected: 0,
                tasks: vec![
                    Task::new(
                        "Infos système",
                        "CPU, RAM, disque, version macOS",
                        TaskAction::SystemInfo,
                        false,
                    ),
                    Task::new(
                        "Nettoyer les caches",
                        "Supprime ~/Library/Caches",
                        TaskAction::CleanCache,
                        false,
                    ),
                    Task::new(
                        "Vider la corbeille",
                        "Supprime le contenu de ~/.Trash",
                        TaskAction::CleanTrash,
                        false,
                    ),
                    Task::new(
                        "Nettoyer les logs",
                        "Supprime ~/Library/Logs",
                        TaskAction::CleanLogs,
                        false,
                    ),
                    Task::new(
                        "Supprimer les .DS_Store",
                        "Recherche récursive depuis ~",
                        TaskAction::CleanDsStore,
                        false,
                    ),
                    Task::new(
                        "Flush DNS",
                        "Vide le cache DNS du système",
                        TaskAction::FlushDns,
                        true,
                    ),
                    Task::new(
                        "Libérer l'espace disque",
                        "Purge les snapshots Time Machine locaux",
                        TaskAction::FreeSpace,
                        false,
                    ),
                ],
            },
            Tab {
                name: " Nettoyage ",
                selected: 0,
                tasks: vec![
                    Task::new(
                        "Nettoyer les caches",
                        "Supprime ~/Library/Caches (caches utilisateur)",
                        TaskAction::CleanCache,
                        false,
                    ),
                    Task::new(
                        "Vider la corbeille",
                        "Supprime ~/.Trash",
                        TaskAction::CleanTrash,
                        false,
                    ),
                    Task::new(
                        "Vider toutes les corbeilles",
                        "~/.Trash + corbeilles des volumes montés",
                        TaskAction::CleanAllTrashes,
                        false,
                    ),
                    Task::new(
                        "Nettoyer les logs",
                        "Supprime ~/Library/Logs",
                        TaskAction::CleanLogs,
                        false,
                    ),
                    Task::new(
                        "Supprimer les .DS_Store",
                        "Supprime les fichiers .DS_Store dans ~",
                        TaskAction::CleanDsStore,
                        false,
                    ),
                    Task::new(
                        "Nettoyer Homebrew",
                        "brew cleanup --prune=all",
                        TaskAction::CleanHomebrew,
                        false,
                    ),
                    Task::new(
                        "Nettoyer Xcode",
                        "DerivedData, DeviceSupport, Simulateurs",
                        TaskAction::CleanXcode,
                        false,
                    ),
                ],
            },
            Tab {
                name: " Système ",
                selected: 0,
                tasks: vec![
                    Task::new(
                        "Infos système",
                        "CPU, RAM, disque, version macOS, uptime",
                        TaskAction::SystemInfo,
                        false,
                    ),
                    Task::new(
                        "Vérifier les mises à jour",
                        "softwareupdate -l",
                        TaskAction::CheckUpdates,
                        false,
                    ),
                    Task::new(
                        "Apps les plus volumineuses",
                        "Scan de /Applications (top 15)",
                        TaskAction::ListLargeApps,
                        false,
                    ),
                    Task::new(
                        "Détecter les gros fichiers",
                        "Fichiers > 500 Mo dans ~",
                        TaskAction::FindLargeFiles,
                        false,
                    ),
                    Task::new(
                        "Flush DNS",
                        "dscacheutil -flushcache + killall mDNSResponder",
                        TaskAction::FlushDns,
                        true,
                    ),
                    Task::new(
                        "Libérer l'espace disque",
                        "tmutil thinlocalsnapshots",
                        TaskAction::FreeSpace,
                        false,
                    ),
                    Task::new(
                        "Réindexer Spotlight",
                        "mdutil -E /",
                        TaskAction::ReindexSpotlight,
                        true,
                    ),
                    Task::new(
                        "Purger la RAM",
                        "Libère la mémoire inactive (sudo purge)",
                        TaskAction::PurgeRam,
                        true,
                    ),
                ],
            },
            Tab {
                name: " Démarrage ",
                selected: 0,
                tasks: vec![
                    Task::new(
                        "Vue détaillée des agents",
                        "Label, statut, tag Apple/Tiers",
                        TaskAction::ListLoginItemsDetailed,
                        false,
                    ),
                    Task::new(
                        "Agents tiers uniquement",
                        "Filtre les agents non-Apple",
                        TaskAction::ListThirdPartyAgents,
                        false,
                    ),
                    Task::new(
                        "Désactiver les agents tiers",
                        "launchctl unload (sans supprimer)",
                        TaskAction::DisableThirdPartyAgents,
                        false,
                    ),
                    Task::new(
                        "Réactiver les agents tiers",
                        "launchctl load",
                        TaskAction::EnableThirdPartyAgents,
                        false,
                    ),
                    Task::new(
                        "Supprimer les LaunchAgents utilisateur",
                        "Décharge puis supprime ~/Library/LaunchAgents",
                        TaskAction::RemoveUserLaunchAgents,
                        false,
                    ),
                    Task::new(
                        "Supprimer tous les LaunchAgents/Daemons",
                        "Décharge puis supprime agents + daemons",
                        TaskAction::RemoveAllLaunchItems,
                        true,
                    ),
                ],
            },
        ];

        let mut log = vec!["══ Infos système ══".to_string()];
        log.extend(startup_log);
        log.push(String::new());
        log.push("Sélectionnez une tâche et appuyez sur Entrée. (d: simulation)".to_string());

        Self {
            tabs,
            active_tab: 0,
            log,
            log_scroll: 0,
            dry_run: false,
            confirming: false,
        }
    }

    pub fn toggle_dry_run(&mut self) {
        self.dry_run = !self.dry_run;
        utils::set_dry_run(self.dry_run);
        if self.dry_run {
            self.log.push("Mode SIMULATION activé — aucune modification ne sera effectuée.".into());
        } else {
            self.log.push("Mode simulation désactivé — exécution réelle.".into());
        }
        self.log_scroll = self.log.len().saturating_sub(1);
    }

    pub fn next_tab(&mut self) {
        self.confirming = false;
        self.active_tab = (self.active_tab + 1) % self.tabs.len();
    }

    pub fn prev_tab(&mut self) {
        self.confirming = false;
        if self.active_tab == 0 {
            self.active_tab = self.tabs.len() - 1;
        } else {
            self.active_tab -= 1;
        }
    }

    pub fn next_task(&mut self) {
        self.confirming = false;
        let tab = &mut self.tabs[self.active_tab];
        if !tab.tasks.is_empty() {
            tab.selected = (tab.selected + 1) % tab.tasks.len();
        }
    }

    pub fn prev_task(&mut self) {
        self.confirming = false;
        let tab = &mut self.tabs[self.active_tab];
        if !tab.tasks.is_empty() {
            if tab.selected == 0 {
                tab.selected = tab.tasks.len() - 1;
            } else {
                tab.selected -= 1;
            }
        }
    }

    pub fn toggle_task(&mut self) {
        self.confirming = false;
        let tab = &mut self.tabs[self.active_tab];
        if let Some(task) = tab.tasks.get_mut(tab.selected) {
            task.checked = !task.checked;
        }
    }

    pub fn cancel_confirm(&mut self) {
        if self.confirming {
            self.confirming = false;
            self.log.push("Annulé.".into());
            self.log_scroll = self.log.len().saturating_sub(1);
        }
    }

    pub fn execute_current_task(&mut self) {
        let tab = &self.tabs[self.active_tab];
        let selected = tab.selected;
        let task = match tab.tasks.get(selected) {
            Some(t) => t,
            None => return,
        };

        let action = task.action;

        // Confirmation pour tâches destructives (sauf en dry-run)
        if action.is_destructive() && !self.dry_run && !self.confirming {
            self.confirming = true;
            self.log.push(format!("Confirmer \"{}\" ? (Entrée = oui, Esc = non)", task.name));
            self.log_scroll = self.log.len().saturating_sub(1);
            return;
        }

        self.confirming = false;

        let task_name = task.name;

        if self.dry_run {
            self.log.push(format!("─── {} [SIMULATION] ───", task_name));
        } else {
            self.log.push(format!("─── {} ───", task_name));
        }

        let results = execute_action(action);
        let has_error = results.iter().any(|r| r.contains("Erreur") || r.contains("sudo"));
        for msg in results {
            self.log.push(msg);
        }

        let tab = &mut self.tabs[self.active_tab];
        if let Some(task) = tab.tasks.get_mut(selected) {
            task.status = if has_error {
                TaskStatus::Error
            } else {
                TaskStatus::Done
            };
        }
        self.log_scroll = self.log.len().saturating_sub(1);
    }

    pub fn run_selected(&mut self) {
        self.confirming = false;
        let tab = &mut self.tabs[self.active_tab];
        let checked_actions: Vec<(usize, TaskAction, &'static str)> = tab
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.checked)
            .map(|(i, t)| (i, t.action, t.name))
            .collect();

        if checked_actions.is_empty() {
            self.log
                .push("Aucune tâche sélectionnée. Utilisez Espace pour sélectionner.".into());
            return;
        }

        if self.dry_run {
            self.log.push("═══ Exécution groupée [SIMULATION] ═══".into());
        } else {
            self.log.push("═══ Exécution groupée ═══".into());
        }

        for (idx, action, name) in checked_actions {
            self.log.push(format!("─── {} ───", name));
            let results = execute_action(action);
            let has_error = results
                .iter()
                .any(|r| r.contains("Erreur") || r.contains("sudo"));
            for msg in results {
                self.log.push(msg);
            }
            self.tabs[self.active_tab].tasks[idx].status = if has_error {
                TaskStatus::Error
            } else {
                TaskStatus::Done
            };
            self.tabs[self.active_tab].tasks[idx].checked = false;
        }
        self.log.push("═══ Terminé ═══".into());
        self.log_scroll = self.log.len().saturating_sub(1);
    }

    pub fn scroll_log_up(&mut self) {
        self.log_scroll = self.log_scroll.saturating_sub(1);
    }

    pub fn scroll_log_down(&mut self) {
        if self.log_scroll < self.log.len().saturating_sub(1) {
            self.log_scroll += 1;
        }
    }
}

fn execute_action(action: TaskAction) -> Vec<String> {
    match action {
        // Infos
        TaskAction::SystemInfo => sysinfo::get_system_info(),
        TaskAction::ListLargeApps => scanner::list_large_apps(),
        TaskAction::FindLargeFiles => scanner::find_large_files(),
        TaskAction::CheckUpdates => system::check_updates(),
        // Nettoyage
        TaskAction::CleanCache => cache::clean_cache(),
        TaskAction::CleanTrash => trash::clean_trash(),
        TaskAction::CleanAllTrashes => trash::clean_all_trashes(),
        TaskAction::CleanLogs => logs::clean_logs(),
        TaskAction::CleanDsStore => dsstore::clean_dsstore(),
        TaskAction::CleanHomebrew => homebrew::clean_homebrew(),
        TaskAction::CleanXcode => xcode::clean_xcode(),
        // Système
        TaskAction::FlushDns => system::flush_dns(),
        TaskAction::FreeSpace => system::free_purgeable_space(),
        TaskAction::ReindexSpotlight => system::reindex_spotlight(),
        TaskAction::PurgeRam => system::purge_ram(),
        // Démarrage
        TaskAction::ListLoginItemsDetailed => login::list_login_items_detailed(),
        TaskAction::ListThirdPartyAgents => login::list_third_party_agents(),
        TaskAction::DisableThirdPartyAgents => login::disable_third_party_agents(),
        TaskAction::EnableThirdPartyAgents => login::enable_third_party_agents(),
        TaskAction::RemoveUserLaunchAgents => launch::remove_user_launch_agents(),
        TaskAction::RemoveAllLaunchItems => launch::remove_all_launch_items(),
    }
}
