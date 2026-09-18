//! Tâches de maintenance, indépendantes de l'affichage.

pub mod agents;
pub mod clean;
pub mod docker;
pub mod maintenance;
pub mod scan;

use serde::Serialize;

use crate::ui::Printer;

/// Contexte partagé par toutes les tâches.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    /// Ne rien modifier, seulement mesurer.
    pub dry_run: bool,
    /// Répondre « oui » à toutes les confirmations.
    pub yes: bool,
    /// Sortie machine : aucune confirmation interactive possible.
    pub json: bool,
    pub printer: Printer,
}

impl Ctx {
    /// Demande confirmation, sauf en simulation ou si `--yes` est passé.
    pub fn confirm(&self, question: &str) -> bool {
        if self.dry_run || self.yes {
            return true;
        }
        if self.json {
            self.printer
                .error("confirmation requise : ajoutez --yes en mode --json.");
            return false;
        }
        self.printer.confirm(question)
    }
}

/// État final d'une opération.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Effectuée.
    Ok,
    /// Simulée (`--dry-run`).
    Simulated,
    /// Non applicable (outil absent, dossier vide, refus de l'utilisateur).
    Skipped,
    /// Échec.
    Failed,
}

impl Status {
    /// Statut d'une opération réussie, selon le mode.
    pub fn done(dry_run: bool) -> Self {
        if dry_run { Self::Simulated } else { Self::Ok }
    }

    pub fn is_failure(self) -> bool {
        self == Self::Failed
    }
}

/// Résultat générique d'une action système.
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub action: String,
    pub status: Status,
    pub messages: Vec<String>,
}

impl Outcome {
    pub fn new(action: impl Into<String>, status: Status) -> Self {
        Self {
            action: action.into(),
            status,
            messages: Vec::new(),
        }
    }

    pub fn ok(action: impl Into<String>, dry_run: bool) -> Self {
        Self::new(action, Status::done(dry_run))
    }

    pub fn skipped(action: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::new(action, Status::Skipped).with(reason)
    }

    pub fn failed(action: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::new(action, Status::Failed).with(reason)
    }

    pub fn with(mut self, message: impl Into<String>) -> Self {
        self.messages.push(message.into());
        self
    }

    pub fn push(&mut self, message: impl Into<String>) {
        self.messages.push(message.into());
    }

    /// Affiche le résultat au format texte.
    pub fn render(&self, printer: &Printer) {
        match self.status {
            Status::Ok => printer.success(&self.action),
            Status::Simulated => printer.success(format!("{} [simulation]", self.action)),
            Status::Skipped => printer.skipped(&self.action),
            Status::Failed => printer.error(&self.action),
        }
        for message in &self.messages {
            printer.item(printer.dim(message));
        }
    }
}
