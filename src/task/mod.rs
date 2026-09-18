//! Maintenance tasks, independent of how results are displayed.

pub mod agents;
pub mod clean;
pub mod dev;
pub mod docker;
pub mod journal;
pub mod maintenance;
pub mod orphans;
pub mod ram;
pub mod scan;
pub mod schedule;
pub mod sizes;

use serde::Serialize;

use crate::config::Config;
use crate::sys::fsx::{Disposal, Policy};
use crate::ui::Printer;

/// Context shared by every task.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// Measure only, change nothing.
    pub dry_run: bool,
    /// Answer yes to every confirmation.
    pub yes: bool,
    /// Machine-readable output: no interactive prompt is possible.
    pub json: bool,
    /// Remove for good instead of moving to the trash.
    pub purge: bool,
    /// Leave alone anything touched more recently than this many days.
    pub min_age_days: u64,
    /// Exclusions, defaults and safety settings.
    pub config: Config,
    pub printer: Printer,
}

impl Ctx {
    /// What a deletion does in this run.
    pub fn disposal(&self) -> Disposal {
        if self.purge {
            Disposal::Purge
        } else {
            self.config.disposal
        }
    }

    /// The removal policy this run must obey.
    pub fn policy(&self) -> Policy<'_> {
        Policy::new(self.dry_run, self.disposal(), &self.config.exclude)
    }

    /// Asks for confirmation, unless simulating or `--yes` was passed.
    pub fn confirm(&self, question: &str) -> bool {
        if self.dry_run || self.yes {
            return true;
        }
        if self.json {
            self.printer
                .error("confirmation required: add --yes when using --json.");
            return false;
        }
        self.printer.confirm(question)
    }

    /// Second gate, for what cannot be undone.
    ///
    /// A `y` given in a hurry is cheap; typing the word is not. This is only
    /// asked when the run really is irreversible — a deletion that goes to the
    /// trash stops at the first confirmation.
    pub fn confirm_final(&self, warning: &str, phrase: &str) -> bool {
        if self.dry_run {
            return true;
        }
        if !self.config.confirm_twice {
            return true;
        }

        if self.yes {
            // Scripted, but the user still deserves to see it in the log.
            self.printer.warn(format!("{warning} — allowed by --yes."));
            return true;
        }
        if self.json {
            self.printer
                .error("confirmation required: add --yes when using --json.");
            return false;
        }

        self.printer.warn(warning);
        self.printer.confirm_typed(phrase)
    }
}

/// Final state of an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Carried out.
    Ok,
    /// Simulated (`--dry-run`).
    Simulated,
    /// Not applicable (missing tool, empty directory, user declined).
    Skipped,
    /// Failed.
    Failed,
}

impl Status {
    /// Status of a successful operation, depending on the mode.
    pub fn done(dry_run: bool) -> Self {
        if dry_run { Self::Simulated } else { Self::Ok }
    }

    pub fn is_failure(self) -> bool {
        self == Self::Failed
    }
}

/// Generic result of a system action.
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

    /// Renders the outcome as text.
    pub fn render(&self, printer: &Printer) {
        match self.status {
            Status::Ok => printer.success(&self.action),
            Status::Simulated => printer.success(format!("{} [dry run]", self.action)),
            Status::Skipped => printer.skipped(&self.action),
            Status::Failed => printer.error(&self.action),
        }
        for message in &self.messages {
            printer.item(printer.dim(message));
        }
    }
}
