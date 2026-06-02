//! Installs the repo's git hooks (`.githooks/`) for the current checkout.
//!
//! Triggered as a normal build script during `cargo build`/`cargo test`. It is
//! deliberately tolerant: anything unexpected (no git, no `.githooks/`, a
//! read-only hooks dir) is a quiet no-op so it never breaks a build — worst
//! case the hook simply isn't installed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

/// Marker inserted just after the shebang so we only ever overwrite hooks we
/// installed ourselves — never a contributor's own hook or a foreign tool's.
const MARKER: &str = "# managed-by: calimero-git-hooks";

/// Run `git -C <dir> <args...>` and return trimmed stdout, or `None` on any
/// failure (git missing, not a repo, non-zero exit, empty output).
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let out = String::from_utf8(output.stdout).ok()?;
    let out = out.trim().to_owned();
    (!out.is_empty()).then_some(out)
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    // The working-tree root. Absent in published crates / source tarballs that
    // ship without a `.git`, in which case there is nothing to install.
    let Some(toplevel) = git(&manifest_dir, &["rev-parse", "--show-toplevel"]) else {
        return;
    };
    let toplevel = PathBuf::from(toplevel);

    let src_dir = toplevel.join(".githooks");
    // Reinstall whenever the tracked hooks change.
    println!("cargo:rerun-if-changed={}", src_dir.display());
    if !src_dir.is_dir() {
        return;
    }

    // Ask git where hooks actually live for this checkout. Correct for plain
    // clones (<repo>/.git/hooks), linked worktrees (the shared common dir,
    // since `.git` is a file there), and any `core.hooksPath` override.
    let Some(hooks_dir) = git(&toplevel, &["rev-parse", "--git-path", "hooks"]) else {
        return;
    };
    // `--git-path` prints relative to the cwd we passed (the toplevel) for
    // in-tree dirs, but absolute for the shared worktree dir. Normalize.
    let hooks_dir = {
        let path = PathBuf::from(&hooks_dir);
        if path.is_absolute() {
            path
        } else {
            toplevel.join(path)
        }
    };

    if let Err(err) = fs::create_dir_all(&hooks_dir) {
        println!(
            "cargo:warning=git-hooks: cannot create {}: {err}",
            hooks_dir.display()
        );
        return;
    }

    let Ok(entries) = fs::read_dir(&src_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let src = entry.path();
        if !src.is_file() {
            continue;
        }
        println!("cargo:rerun-if-changed={}", src.display());

        let Ok(body) = fs::read_to_string(&src) else {
            continue;
        };
        // Insert the marker right after the shebang line so the script still
        // starts with `#!...`, and we can recognize hooks we manage.
        let managed = match body.split_once('\n') {
            Some((first, rest)) if first.starts_with("#!") => format!("{first}\n{MARKER}\n{rest}"),
            _ => format!("{MARKER}\n{body}"),
        };

        let dst = hooks_dir.join(entry.file_name());
        if let Ok(existing) = fs::read_to_string(&dst) {
            if !existing.contains(MARKER) {
                // Someone else owns this hook; don't clobber it.
                println!(
                    "cargo:warning=git-hooks: leaving {} untouched (not managed by calimero-git-hooks)",
                    dst.display()
                );
                continue;
            }
            if existing == managed {
                continue; // already current
            }
        }

        if let Err(err) = fs::write(&dst, managed.as_bytes()) {
            println!(
                "cargo:warning=git-hooks: failed writing {}: {err}",
                dst.display()
            );
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(0o755));
        }
    }
}
