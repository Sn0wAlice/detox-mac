//! Exécution de commandes externes.

use std::ffi::OsStr;
use std::process::Command;

/// Sortie d'une commande terminée.
#[derive(Debug, Clone)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// Première sortie non vide (stdout, sinon stderr).
    pub fn text(&self) -> &str {
        if self.stdout.is_empty() {
            &self.stderr
        } else {
            &self.stdout
        }
    }
}

/// Lance une commande et renvoie sa sortie quel que soit le code de retour.
pub fn run_raw<S: AsRef<OsStr>>(program: &str, args: &[S]) -> Result<Output, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|err| format!("{program} : {err}"))?;

    Ok(Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

/// Lance une commande, en échouant si son code de retour n'est pas nul.
pub fn run<S: AsRef<OsStr>>(program: &str, args: &[S]) -> Result<Output, String> {
    let output = run_raw(program, args)?;
    if output.code == 0 {
        Ok(output)
    } else if !output.stderr.is_empty() {
        Err(output.stderr)
    } else {
        Err(format!("{program} a échoué (code {})", output.code))
    }
}

/// Indique si un exécutable est présent dans le `PATH`.
pub fn exists(program: &str) -> bool {
    run("which", &[program]).is_ok()
}

/// Valeur d'une clé `sysctl`.
pub fn sysctl(key: &str) -> Option<String> {
    run("sysctl", &["-n", key]).ok().map(|o| o.stdout)
}

/// Indique si le processus tourne avec les privilèges root.
pub fn is_root() -> bool {
    run("id", &["-u"]).map(|o| o.stdout == "0").unwrap_or(false)
}
