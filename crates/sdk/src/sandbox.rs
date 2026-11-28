//! Sandbox runs: what a `#[app::sandbox]` method does with its Linux sandbox.
//!
//! [`exec()`] runs one command in a fresh sandbox over the run's workspace (`/workspace`), as an
//! unprivileged user with no network; [`write()`] and [`read()`] move bytes in and out of the
//! workspace, and [`collect()`] takes a workspace file out as one of the run's sealed outputs. A
//! path is relative to the workspace; one that leaves it, or names a symlink or anything but a
//! regular file, is refused. Nothing carries over from another run.
//!
//! Only a sandbox run has a sandbox: everywhere else, and in a native build, every call answers
//! a `Sandbox` error.

use serde_json::json;

use crate::blob::BlobRef;
use crate::error::{AppError, Result};

/// One command to run.
#[derive(Clone, Debug)]
pub struct Command {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Under the workspace; `None` is `/workspace` itself.
    pub cwd: Option<String>,
    pub stdin: Vec<u8>,
    /// The command and everything it started are killed at it.
    pub timeout_ms: u32,
}

impl Command {
    /// `argv` with no environment, stdin or working directory and a 60 s timeout.
    pub fn new<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            env: Vec::new(),
            cwd: None,
            stdin: Vec::new(),
            timeout_ms: 60_000,
        }
    }

    /// `sh -c <script>`.
    pub fn sh(script: &str) -> Self {
        Self::new(["/bin/sh", "-c", script])
    }

    #[must_use]
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_owned(), value.to_owned()));
        self
    }

    #[must_use]
    pub fn cwd(mut self, dir: &str) -> Self {
        self.cwd = Some(dir.to_owned());
        self
    }

    #[must_use]
    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = bytes.into();
        self
    }

    #[must_use]
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }
}

/// How a command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// The exit code, or 128 + n for a command killed by signal n.
    pub exit_code: i32,
    /// Up to 1 MiB each; `truncated` says the rest was dropped.
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
    pub timed_out: bool,
    pub wall_us: u64,
}

impl Output {
    /// Exited 0 within its timeout.
    pub fn success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }

    /// stdout as text, invalid UTF-8 replaced.
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Runs `cmd` in a fresh sandbox and waits for it.
pub fn exec(cmd: &Command) -> Result<Output> {
    backend::exec(cmd).map_err(fail)
}

/// Writes `bytes` to `path` in the workspace, replacing a regular file there.
pub fn write(path: &str, bytes: &[u8]) -> Result<()> {
    backend::write(path, bytes).map_err(fail)
}

/// Reads at most `max` bytes of a file in the workspace.
pub fn read(path: &str, max: u32) -> Result<Vec<u8>> {
    backend::read(path, max).map_err(fail)
}

/// Takes `path` out of the workspace as one of the run's sealed outputs; the run's result
/// method keeps it by storing the [`BlobRef`].
pub fn collect(path: &str) -> Result<BlobRef> {
    let id = backend::collect(path).map_err(fail)?;
    BlobRef::parse(&id).ok_or_else(|| fail(format!("the host answered a bad file id {id:?}")))
}

fn fail(message: String) -> AppError {
    AppError::with_fields("Sandbox", json!({ "message": message }))
}

#[cfg(all(target_arch = "wasm32", calimero_sandbox))]
mod backend {
    use super::{Command, Output};
    use crate::bindings::calimero::app::sandbox;

    pub fn exec(cmd: &Command) -> Result<Output, String> {
        let r = sandbox::exec(&sandbox::ExecRequest {
            argv: cmd.argv.clone(),
            env: cmd.env.clone(),
            cwd: cmd.cwd.clone(),
            stdin: cmd.stdin.clone(),
            timeout_ms: cmd.timeout_ms,
        })?;
        Ok(Output {
            exit_code: r.exit_code,
            stdout: r.stdout,
            stderr: r.stderr,
            truncated: r.truncated,
            timed_out: r.timed_out,
            wall_us: r.wall_us,
        })
    }

    pub fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
        sandbox::write(path, bytes)
    }

    pub fn read(path: &str, max: u32) -> Result<Vec<u8>, String> {
        sandbox::read(path, max)
    }

    pub fn collect(path: &str) -> Result<String, String> {
        sandbox::collect(path)
    }
}

#[cfg(not(all(target_arch = "wasm32", calimero_sandbox)))]
mod backend {
    use super::{Command, Output};

    const NO_SANDBOX: &str = "no sandbox: only a sandbox run has one";

    pub fn exec(_cmd: &Command) -> Result<Output, String> {
        Err(NO_SANDBOX.to_owned())
    }

    pub fn write(_path: &str, _bytes: &[u8]) -> Result<(), String> {
        Err(NO_SANDBOX.to_owned())
    }

    pub fn read(_path: &str, _max: u32) -> Result<Vec<u8>, String> {
        Err(NO_SANDBOX.to_owned())
    }

    pub fn collect(_path: &str) -> Result<String, String> {
        Err(NO_SANDBOX.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_a_sandbox_run_every_call_is_a_sandbox_error() {
        let err = exec(&Command::sh("true")).unwrap_err();
        assert_eq!(err.code, "Sandbox");
        assert!(write("a", b"x").is_err());
        assert!(read("a", 1).is_err());
        assert!(collect("a").is_err());
    }

    #[test]
    fn a_command_builds_up_its_request() {
        let cmd = Command::new(["python3", "x.py"])
            .env("A", "1")
            .cwd("src")
            .stdin(b"in".to_vec())
            .timeout_ms(5);
        assert_eq!(cmd.argv, ["python3", "x.py"]);
        assert_eq!(cmd.env, [("A".to_owned(), "1".to_owned())]);
        assert_eq!(
            (cmd.cwd.as_deref(), cmd.stdin.as_slice(), cmd.timeout_ms),
            (Some("src"), &b"in"[..], 5)
        );
        assert_eq!(Command::sh("ls").argv, ["/bin/sh", "-c", "ls"]);
    }
}
