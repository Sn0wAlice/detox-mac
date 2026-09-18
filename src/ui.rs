//! Sortie terminal : couleurs, mise en forme, confirmations.

use std::io::{self, IsTerminal, Write};

/// Politique de coloration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorChoice {
    /// Couleurs si la sortie est un terminal.
    Auto,
    /// Toujours colorer.
    Always,
    /// Jamais de couleurs.
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

/// Écrit sur la sortie standard en respectant les options globales.
#[derive(Debug, Clone, Copy)]
pub struct Printer {
    color: bool,
    quiet: bool,
}

impl Printer {
    pub fn new(choice: ColorChoice, quiet: bool) -> Self {
        let color = match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                std::env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal()
            }
        };
        Self { color, quiet }
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

    /// Titre de section.
    pub fn heading(&self, text: &str) {
        self.line("");
        self.line(self.paint(BOLD, text));
    }

    /// Ligne d'information simple.
    pub fn info(&self, text: impl AsRef<str>) {
        self.line(text);
    }

    /// Paire clé / valeur alignée.
    pub fn field(&self, key: &str, value: impl AsRef<str>) {
        self.line(format!("  {:<16} {}", self.dim(key), value.as_ref()));
    }

    /// Ligne de liste.
    pub fn item(&self, text: impl AsRef<str>) {
        self.line(format!("  {}", text.as_ref()));
    }

    /// Succès.
    pub fn success(&self, text: impl AsRef<str>) {
        self.line(format!("{} {}", self.paint(GREEN, "✓"), text.as_ref()));
    }

    /// Opération ignorée.
    pub fn skipped(&self, text: impl AsRef<str>) {
        self.line(format!("{} {}", self.paint(BLUE, "–"), text.as_ref()));
    }

    /// Avertissement : toujours affiché, sur stderr.
    pub fn warn(&self, text: impl AsRef<str>) {
        eprintln!("{} {}", self.paint(YELLOW, "!"), text.as_ref());
    }

    /// Erreur : toujours affichée, sur stderr.
    pub fn error(&self, text: impl AsRef<str>) {
        eprintln!("{} {}", self.paint(RED, "✗"), text.as_ref());
    }

    /// Bandeau du mode simulation.
    pub fn dry_run_banner(&self) {
        self.line(self.paint(
            YELLOW,
            "◆ Mode simulation — aucune modification ne sera faite.",
        ));
    }

    /// Demande une confirmation interactive. Renvoie `false` hors terminal.
    pub fn confirm(&self, question: &str) -> bool {
        if !io::stdin().is_terminal() {
            self.error(format!(
                "{question} — entrée non interactive, utilisez --yes pour confirmer."
            ));
            return false;
        }

        print!("{} {} [o/N] ", self.paint(YELLOW, "?"), question);
        let _ = io::stdout().flush();

        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).is_err() {
            return false;
        }

        matches!(
            answer.trim().to_ascii_lowercase().as_str(),
            "o" | "oui" | "y" | "yes"
        )
    }
}
