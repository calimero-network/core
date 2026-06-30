//! Resolve secret CLI inputs without exposing them in the process's argv.
//!
//! Passing a secret as a positional/inline CLI argument leaks it via shell
//! history, `ps`, and `/proc/<pid>/cmdline`. Instead of taking the secret
//! itself, secret-bearing flags take a *source spec* that this module resolves:
//!
//! - `env:NAME`  — read the value from environment variable `NAME`
//! - `file:PATH` — read the value from `PATH` (trailing newline trimmed)
//! - `-`         — read a single line from stdin
//!
//! A bare value (anything not matching the forms above) is still accepted for
//! backwards compatibility, but emits a deprecation warning recommending the
//! safe forms. When no value is supplied at all and stdin is a TTY, the user is
//! prompted without echo.

use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};

use eyre::{eyre, Result, WrapErr};

/// Resolve an optional secret source spec into the secret value.
///
/// Returns `Ok(None)` only when `arg` is `None` and stdin is not a TTY (so a
/// non-interactive run with no secret supplied stays "not provided" rather than
/// blocking on a prompt). `prompt` labels the no-echo prompt.
pub fn resolve_optional_secret(arg: Option<&str>, prompt: &str) -> Result<Option<String>> {
    match arg {
        Some(spec) => resolve_spec(spec).map(Some),
        None => {
            if io::stdin().is_terminal() {
                prompt_hidden(prompt).map(Some)
            } else {
                Ok(None)
            }
        }
    }
}

/// Resolve a required secret source spec into the secret value.
///
/// Like [`resolve_optional_secret`] but errors when nothing is supplied in a
/// non-interactive context instead of returning `None`.
pub fn resolve_required_secret(arg: Option<&str>, prompt: &str) -> Result<String> {
    match resolve_optional_secret(arg, prompt)? {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) => Err(eyre!("{prompt}: resolved to an empty value")),
        None => Err(eyre!(
            "{prompt}: no value supplied. Provide one via `env:NAME`, `file:PATH`, `-` (stdin), or run interactively."
        )),
    }
}

fn resolve_spec(spec: &str) -> Result<String> {
    if let Some(name) = spec.strip_prefix("env:") {
        let value = env::var(name).wrap_err_with(|| {
            format!("failed to read secret from environment variable `{name}`")
        })?;
        return Ok(value);
    }

    if let Some(path) = spec.strip_prefix("file:") {
        let raw = fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read secret from file `{path}`"))?;
        // Trim a single trailing newline (and any \r) so `echo secret > f` works.
        return Ok(raw.trim_end_matches(['\n', '\r']).to_owned());
    }

    if spec == "-" {
        let mut line = String::new();
        let n = io::stdin()
            .lock()
            .read_line(&mut line)
            .wrap_err("failed to read secret from stdin")?;
        if n == 0 {
            return Err(eyre!("no secret received on stdin"));
        }
        return Ok(line.trim_end_matches(['\n', '\r']).to_owned());
    }

    // Bare literal: keep working, but warn — it is exposed in argv.
    eprintln!(
        "warning: passing a secret directly on the command line exposes it via shell history, \
         `ps`, and /proc. Prefer `env:NAME`, `file:PATH`, or `-` (stdin)."
    );
    Ok(spec.to_owned())
}

fn prompt_hidden(prompt: &str) -> Result<String> {
    // Write the prompt to stderr so it does not pollute machine-readable stdout.
    let mut stderr = io::stderr();
    write!(stderr, "{prompt}: ").wrap_err("failed to write prompt")?;
    stderr.flush().ok();
    let value = rpassword::read_password().wrap_err("failed to read secret from prompt")?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_spec_reads_environment_variable() {
        // SAFETY: single-threaded test; unique var name avoids cross-test races.
        env::set_var("MEROCTL_TEST_SECRET_ENV", "s3cr3t");
        assert_eq!(
            resolve_spec("env:MEROCTL_TEST_SECRET_ENV").unwrap(),
            "s3cr3t"
        );
        env::remove_var("MEROCTL_TEST_SECRET_ENV");
    }

    #[test]
    fn env_spec_errors_when_unset() {
        assert!(resolve_spec("env:MEROCTL_TEST_DEFINITELY_UNSET").is_err());
    }

    #[test]
    fn file_spec_reads_and_trims_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        fs::write(&path, "deadbeef\n").unwrap();
        let spec = format!("file:{}", path.display());
        assert_eq!(resolve_spec(&spec).unwrap(), "deadbeef");
    }

    #[test]
    fn file_spec_errors_on_missing_file() {
        assert!(resolve_spec("file:/no/such/meroctl/secret").is_err());
    }

    #[test]
    fn bare_value_is_returned_verbatim() {
        // (Emits a deprecation warning to stderr.)
        assert_eq!(resolve_spec("literal-secret").unwrap(), "literal-secret");
    }

    #[test]
    fn required_secret_errors_on_empty_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty");
        fs::write(&path, "").unwrap();
        let spec = format!("file:{}", path.display());
        assert!(resolve_required_secret(Some(&spec), "test secret").is_err());
    }
}
