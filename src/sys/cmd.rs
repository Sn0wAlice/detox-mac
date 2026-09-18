//! Running external commands.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Output of a finished command.
#[derive(Debug, Clone)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// The first non-empty stream (stdout, otherwise stderr).
    pub fn text(&self) -> &str {
        if self.stdout.is_empty() {
            &self.stderr
        } else {
            &self.stdout
        }
    }
}

fn capture(output: std::process::Output) -> Output {
    Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
}

fn check(program: &str, output: Output) -> Result<Output, String> {
    if output.code == 0 {
        Ok(output)
    } else if !output.stderr.is_empty() {
        Err(output.stderr)
    } else {
        Err(format!("{program} failed (exit code {})", output.code))
    }
}

/// Runs a command and returns its output whatever the exit code.
pub fn run_raw<S: AsRef<OsStr>>(program: &str, args: &[S]) -> Result<Output, String> {
    Command::new(program)
        .args(args)
        .output()
        .map(capture)
        .map_err(|err| format!("{program}: {err}"))
}

/// Runs a command, failing when its exit code is not zero.
pub fn run<S: AsRef<OsStr>>(program: &str, args: &[S]) -> Result<Output, String> {
    check(program, run_raw(program, args)?)
}

/// Runs a command from a given directory, failing when it fails.
pub fn run_in<S: AsRef<OsStr>>(dir: &Path, program: &str, args: &[S]) -> Result<Output, String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .map(capture)
        .map_err(|err| format!("{program}: {err}"))?;

    check(program, output)
}

/// Whether an executable is reachable through `PATH`.
pub fn exists(program: &str) -> bool {
    run("which", &[program]).is_ok()
}

/// Value of a `sysctl` key.
pub fn sysctl(key: &str) -> Option<String> {
    run("sysctl", &["-n", key]).ok().map(|o| o.stdout)
}

/// Whether the process runs with root privileges.
pub fn is_root() -> bool {
    run("id", &["-u"]).map(|o| o.stdout == "0").unwrap_or(false)
}
