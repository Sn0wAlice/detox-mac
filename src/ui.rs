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

    /// Asks the user to type a word exactly. A stray `y` cannot get through
    /// this one, which is the whole point of asking twice.
    pub fn confirm_typed(&self, phrase: &str) -> bool {
        let Some(answer) = self.ask(&format!(
            "Type {} to confirm, anything else to cancel:",
            self.bold(phrase)
        )) else {
            return false;
        };
        answer.trim() == phrase
    }

    /// Reads one line from the user. `None` outside a terminal.
    pub fn ask(&self, prompt: &str) -> Option<String> {
        if !io::stdin().is_terminal() {
            self.error(format!(
                "{prompt} — not an interactive terminal, pass --yes to confirm."
            ));
            return None;
        }

        print!("{} {} ", self.paint(YELLOW, "?"), prompt);
        let _ = io::stdout().flush();

        let mut answer = String::new();
        match io::stdin().read_line(&mut answer) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(answer.trim().to_string()),
        }
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

/// Parses a list of numbers and ranges: `1,4-6,9`.
///
/// Returns zero-based positions, rejecting anything out of range rather than
/// quietly ignoring it — a typo in a list of things to keep must not end with
/// them being removed.
pub fn parse_ranges(input: &str, count: usize) -> Result<Vec<usize>, String> {
    let mut chosen = Vec::new();

    for piece in input
        .split([',', ' '])
        .filter(|part| !part.trim().is_empty())
    {
        let piece = piece.trim();
        let (start, end) = match piece.split_once('-') {
            Some((start, end)) => (start.trim(), end.trim()),
            None => (piece, piece),
        };

        let start: usize = start
            .parse()
            .map_err(|_| format!("`{piece}` is not a number or a range"))?;
        let end: usize = end
            .parse()
            .map_err(|_| format!("`{piece}` is not a number or a range"))?;

        if start == 0 || end == 0 || start > count || end > count {
            return Err(format!("`{piece}` is outside 1–{count}"));
        }
        if start > end {
            return Err(format!("`{piece}` runs backwards"));
        }

        for number in start..=end {
            if !chosen.contains(&(number - 1)) {
                chosen.push(number - 1);
            }
        }
    }

    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_numbers_and_ranges() {
        assert_eq!(parse_ranges("1,4-6", 10).unwrap(), vec![0, 3, 4, 5]);
        assert_eq!(parse_ranges("2 3", 10).unwrap(), vec![1, 2]);
        assert_eq!(parse_ranges("", 10).unwrap(), Vec::<usize>::new());
    }

    #[test]
    fn refuses_what_it_cannot_honour() {
        // Silently dropping a bad entry would delete what the user meant to keep.
        assert!(parse_ranges("11", 10).is_err());
        assert!(parse_ranges("0", 10).is_err());
        assert!(parse_ranges("6-2", 10).is_err());
        assert!(parse_ranges("all", 10).is_err());
    }
}
