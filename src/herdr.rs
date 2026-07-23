//! herdr query seam: shell out to the `herdr` CLI, behind traits so tests never
//! spawn a real process. Ported from the herdr-file-viewer plugin's pattern
//! (unix-only here; platforms exclude Windows).

use std::ffi::{OsStr, OsString};
use std::io;
use std::process::{Command, Output};

/// The substitution point the app depends on: run a herdr subcommand expected
/// to emit JSON on stdout.
pub trait HerdrCli {
    fn run_json(&self, args: &[&str]) -> io::Result<String>;
}

/// Inner seam: lets tests assert argv without real spawning.
pub trait CommandRunner {
    fn run(&self, program: &OsStr, args: &[&str]) -> io::Result<Output>;
}

/// Real command execution via `std::process::Command`.
pub struct RealRunner;
impl CommandRunner for RealRunner {
    fn run(&self, program: &OsStr, args: &[&str]) -> io::Result<Output> {
        Command::new(program).args(args).output()
    }
}

/// The live herdr adapter.
pub struct LiveHerdr<R: CommandRunner = RealRunner> {
    program: OsString,
    runner: R,
}

impl LiveHerdr<RealRunner> {
    /// Resolve `herdr` from `$HERDR_BIN_PATH` (or `"herdr"` on PATH).
    pub fn from_env() -> Self {
        Self {
            program: resolve_program(std::env::var("HERDR_BIN_PATH").ok()),
            runner: RealRunner,
        }
    }
}

impl<R: CommandRunner> LiveHerdr<R> {
    pub fn with_runner(program: impl Into<OsString>, runner: R) -> Self {
        Self { program: program.into(), runner }
    }
}

impl<R: CommandRunner> HerdrCli for LiveHerdr<R> {
    fn run_json(&self, args: &[&str]) -> io::Result<String> {
        let out = self.runner.run(&self.program, args)?;
        if !out.status.success() {
            return Err(io::Error::other("herdr exited non-zero"));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// `Some(non-empty)` → that path; `None`/empty → `"herdr"`.
pub fn resolve_program(var: Option<String>) -> OsString {
    match var {
        Some(v) if !v.is_empty() => OsString::from(v),
        _ => OsString::from("herdr"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    struct Fake {
        stdout: String,
        raw_status: i32,
    }
    impl CommandRunner for Fake {
        fn run(&self, _program: &OsStr, _args: &[&str]) -> io::Result<Output> {
            Ok(Output {
                status: ExitStatus::from_raw(self.raw_status),
                stdout: self.stdout.clone().into_bytes(),
                stderr: Vec::new(),
            })
        }
    }

    #[test]
    fn run_json_returns_stdout_on_success() {
        let h = LiveHerdr::with_runner("herdr", Fake { stdout: r#"{"ok":true}"#.into(), raw_status: 0 });
        assert_eq!(h.run_json(&["agent", "list"]).unwrap(), r#"{"ok":true}"#);
    }

    #[test]
    fn run_json_errors_on_nonzero_exit() {
        // from_raw(256) => exit code 1 on unix.
        let h = LiveHerdr::with_runner("herdr", Fake { stdout: String::new(), raw_status: 256 });
        assert!(h.run_json(&["agent", "list"]).is_err());
    }

    #[test]
    fn resolve_program_falls_back_to_herdr() {
        assert_eq!(resolve_program(None), OsString::from("herdr"));
        assert_eq!(resolve_program(Some(String::new())), OsString::from("herdr"));
        assert_eq!(resolve_program(Some("/custom/herdr".into())), OsString::from("/custom/herdr"));
    }
}
