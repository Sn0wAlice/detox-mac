//! Maintenance tasks, independent of how results are displayed.

pub mod agents;
pub mod clean;
pub mod dev;
pub mod docker;
pub mod maintenance;
pub mod ram;
pub mod scan;

use serde::Serialize;

use crate::ui::Printer;

/// Context shared by every task.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    /// Measure only, change nothing.
    pub dry_run: bool,
    /// Answer yes to every confirmation.
    pub yes: bool,
    /// Machine-readable output: no interactive prompt is possible.
    pub json: bool,
    pub printer: Printer,
}

impl Ctx {
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
