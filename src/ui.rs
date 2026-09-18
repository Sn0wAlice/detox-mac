//! Terminal output: colours, progress, confirmations.

use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

/// When to colourise the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorChoice {
    /// Colour when the output is a terminal.
    Auto,
    /// Always colour.
    Always,
    /// Never colour.
    Never,
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const CYAN: &str = "\x1b[36m";

/// Erases the current line and returns to its start.
const CLEAR_LINE: &str = "\x1b[2K\r";

/// Writes to standard output, honouring the global options.
#[derive(Debug, Clone, Copy)]
pub struct Printer {
    color: bool,
    quiet: bool,
    animated: bool,
}

impl Printer {
    pub fn new(choice: ColorChoice, quiet: bool) -> Self {
        let forced = choice == ColorChoice::Always;
        let color = match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                std::env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal()
            }
        };

        Self {
            color,
            quiet,
            // Progress is drawn on stderr, so that is the stream to check.
            animated: !quiet && (forced || io::stderr().is_terminal()),
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("{code}{text}{RESET}")
        } else {
            text.to_string()
        }
    }

    pub fn bold(&self, text: &str) -> String {
        self.paint(BOLD, text)
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint(DIM, text)
    }

    pub fn accent(&self, text: &str) -> String {
        self.paint(CYAN, text)
    }

    fn line(&self, text: impl AsRef<str>) {
        if !self.quiet {
            println!("{}", text.as_ref());
        }
    }

    /// Section title.
    pub fn heading(&self, text: &str) {
        self.line("");
        self.line(self.paint(BOLD, text));
    }

    /// Plain informational line.
    pub fn info(&self, text: impl AsRef<str>) {
        self.line(text);
    }

    /// Aligned key / value pair.
    pub fn field(&self, key: &str, value: impl AsRef<str>) {
        self.line(format!("  {:<16} {}", self.dim(key), value.as_ref()));
    }

    /// List item.
    pub fn item(&self, text: impl AsRef<str>) {
        self.line(format!("  {}", text.as_ref()));
    }

    /// Success.
    pub fn success(&self, text: impl AsRef<str>) {
        self.line(format!("{} {}", self.paint(GREEN, "✓"), text.as_ref()));
    }

    /// Operation that did not apply.
    pub fn skipped(&self, text: impl AsRef<str>) {
        self.line(format!("{} {}", self.paint(BLUE, "–"), text.as_ref()));
    }

    /// Warning: always shown, on stderr.
    pub fn warn(&self, text: impl AsRef<str>) {
        eprintln!("{} {}", self.paint(YELLOW, "!"), text.as_ref());
    }

    /// Error: always shown, on stderr.
    pub fn error(&self, text: impl AsRef<str>) {
        eprintln!("{} {}", self.paint(RED, "✗"), text.as_ref());
    }

    /// Banner for simulation mode.
    pub fn dry_run_banner(&self) {
        self.line(self.paint(YELLOW, "◆ Dry run — nothing will be modified."));
    }

    /// A progress bar with a known number of steps.
    pub fn bar(&self, label: &str, total: usize) -> Progress {
        Progress::new(self.animated, self.color, label, Some(total))
    }

    /// A spinner, for work whose length is unknown upfront.
    pub fn spinner(&self, label: &str) -> Progress {
        Progress::new(self.animated, self.color, label, None)
    }

    /// Asks for confirmation. Returns `false` outside a terminal.
    pub fn confirm(&self, question: &str) -> bool {
        if !io::stdin().is_terminal() {
            self.error(format!(
                "{question} — not an interactive terminal, pass --yes to confirm."
            ));
            return false;
        }

        print!("{} {} [y/N] ", self.paint(YELLOW, "?"), question);
        let _ = io::stdout().flush();

        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).is_err() {
            return false;
        }

        matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    }
}

/// Minimal progress indicator drawn on a single line of stderr.
///
/// Stdout is left untouched so that piping and `--json` keep working.
pub struct Progress {
    enabled: bool,
    color: bool,
    label: String,
    total: Option<usize>,
    done: usize,
    frame: usize,
    last_draw: Option<Instant>,
}

/// Minimum delay between two redraws.
const FRAME_DELAY: Duration = Duration::from_millis(80);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BAR_WIDTH: usize = 24;

impl Progress {
    fn new(enabled: bool, color: bool, label: &str, total: Option<usize>) -> Self {
        Self {
            // A bar with nothing to do would only flicker.
            enabled: enabled && total != Some(0),
            color,
            label: label.to_string(),
            total,
            done: 0,
            frame: 0,
            last_draw: None,
        }
    }

    /// Sets the running count shown by a spinner.
    pub fn done_count(&mut self, done: usize) {
        self.done = done;
    }

    /// Reports progress without completing a step (spinners).
    pub fn tick(&mut self, detail: &str) {
        self.draw(detail, false);
    }

    /// Completes one step.
    pub fn advance(&mut self, detail: &str) {
        self.done += 1;
        self.draw(detail, false);
    }

    /// Erases the progress line.
    pub fn finish(&mut self) {
        if self.enabled && self.last_draw.is_some() {
            eprint!("{CLEAR_LINE}");
            let _ = io::stderr().flush();
        }
        self.last_draw = None;
    }

    fn draw(&mut self, detail: &str, force: bool) {
        if !self.enabled {
            return;
        }
        // Redrawing faster than the eye can follow only wastes syscalls.
        if !force
            && self
                .last_draw
                .is_some_and(|last| last.elapsed() < FRAME_DELAY)
        {
            return;
        }
        self.last_draw = Some(Instant::now());
        self.frame = self.frame.wrapping_add(1);

        let head = match self.total {
            Some(total) => {
                let filled = (self.done * BAR_WIDTH)
                    .div_ceil(total.max(1))
                    .min(BAR_WIDTH);
                format!(
                    "{}{} {}/{}",
                    "█".repeat(filled),
                    "░".repeat(BAR_WIDTH - filled),
                    self.done,
                    total
                )
            }
            None => format!("{} {}", SPINNER[self.frame % SPINNER.len()], self.done),
        };

        let line = format!(
            "{} {}  {}",
            self.label,
            head,
            crate::format::truncate(detail, 48)
        );
        let line = if self.color {
            format!("{DIM}{line}{RESET}")
        } else {
            line
        };

        eprint!("{CLEAR_LINE}{line}");
        let _ = io::stderr().flush();
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.finish();
    }
}
