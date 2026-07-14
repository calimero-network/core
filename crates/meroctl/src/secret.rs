//! Resolve secret CLI inputs without exposing them in the process's argv.
//!
//! Passing a secret as a positional/inline CLI argument leaks it via shell
//! history, `ps`, and `/proc/<pid>/cmdline`. Instead of taking the secret
//! itself, secret-bearing flags take a *source spec* that this module resolves:
//!
//! - `env:NAME`  — read the value from environment variable `NAME`
//! - `file:PATH` — read the value from `PATH` (trailing newlines trimmed)
//! - `-`         — read one line from stdin (secrets here are single-line, e.g.
//!   hex keys or JWTs)
//!
//! A bare value (anything not matching the forms above) is still accepted for
//! backwards compatibility, but emits a deprecation warning recommending the
//! safe forms.
//!
//! Only [`resolve_required_secret`] ever prompts (and only on a TTY); an
//! *optional* secret that is simply omitted resolves to `None` without a
//! prompt, so commands with optional secrets (e.g. `node add` tokens) are not
//! interrupted.

use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};

use eyre::{eyre, Result, WrapErr};

/// Resolve an optional secret source spec into the secret value.
///
/// Returns `Ok(None)` when `arg` is `None` — an omitted optional secret stays
/// "not provided" and never triggers a prompt. When a spec *is* given it is
/// resolved (and a failure to resolve it is an error).
pub fn resolve_optional_secret(arg: Option<&str>) -> Result<Option<String>> {
    match arg {
        Some(spec) => resolve_spec(spec).map(Some),
        None => Ok(None),
    }
}

/// Resolve a required secret source spec into the secret value.
///
/// When a spec is given it is resolved (and rejected if it resolves to empty).
/// When nothing is supplied, a TTY is prompted without echo; a non-interactive
/// run errors rather than hanging.
pub fn resolve_required_secret(arg: Option<&str>, prompt: &str) -> Result<String> {
    match arg {
        Some(spec) => {
            let value = resolve_spec(spec)?;
            if value.is_empty() {
                return Err(eyre!("{prompt}: resolved to an empty value"));
            }
            Ok(value)
        }
        None if io::stdin().is_terminal() => {
            let value = prompt_hidden(prompt)?;
            if value.is_empty() {
                return Err(eyre!("{prompt}: empty value entered"));
            }
            Ok(value)
        }
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
        // Trim trailing newline(s)/CRs so `echo secret > f` works; a secret
        // never legitimately ends in a newline.
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
        // The test harness runs tests on multiple threads, so this mutates the
        // process environment concurrently with other tests. That is sound on
        // edition 2021 (`set_var` is safe) and race-free in practice here: the
        // variable name is unique to this test and no other test reads it.
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
