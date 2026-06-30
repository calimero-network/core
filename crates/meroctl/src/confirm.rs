//! Interactive confirmation for destructive commands.
//!
//! Destructive operations (deleting a context/group/blob, removing members,
//! leaving a group) should not run silently. [`confirm`] gates them behind a
//! TTY prompt, with a `--yes` escape hatch for scripts.

use std::io::{self, BufRead, IsTerminal, Write};

use eyre::{eyre, Result};

/// Ask the user to confirm a destructive action.
///
/// - `assume_yes` (wired to a command's `--yes`) skips the prompt and proceeds.
/// - On an interactive terminal, prints `prompt [y/N]:` and returns whether the
///   user typed an affirmative answer (`y`/`yes`, case-insensitive).
/// - In a **non-interactive** session without `--yes`, returns an error rather
///   than silently proceeding — so a piped/CI invocation can't destroy data by
///   accident.
pub fn confirm(prompt: &str, assume_yes: bool) -> Result<bool> {
    if assume_yes {
        return Ok(true);
    }

    if !io::stdin().is_terminal() {
        return Err(eyre!(
            "{prompt}: refusing to proceed without confirmation in a non-interactive session. \
             Re-run with --yes to confirm."
        ));
    }

    let mut stderr = io::stderr();
    write!(stderr, "{prompt} [y/N]: ").map_err(|e| eyre!("failed to write prompt: {e}"))?;
    stderr.flush().ok();

    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| eyre!("failed to read confirmation: {e}"))?;

    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::confirm;

    #[test]
    fn assume_yes_skips_prompt_and_proceeds() {
        // With --yes we must proceed without touching stdin (works in CI/non-TTY).
        assert!(confirm("delete everything?", true).unwrap());
    }

    #[test]
    fn non_interactive_without_yes_refuses() {
        // Tests run with a non-TTY stdin, so this must error rather than block
        // or silently proceed.
        assert!(confirm("delete everything?", false).is_err());
    }
}
