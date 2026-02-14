//! Shared build script utilities for Calimero binaries and crates.
//!
//! Provides workspace version reading and git metadata so build scripts stay DRY
//! and consistent (e.g. correct git_dir resolution for rerun-if-changed).

use std::path::Path;
use std::process::Command;

/// Read `[workspace.metadata.workspaces].version` from the workspace root Cargo.toml.
/// Used so binaries and crates get the release version instead of the workspace
/// placeholder `0.0.0`. Returns `None` if not found (e.g. not in a workspace).
pub fn read_workspace_version() -> Option<String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let mut path = std::path::PathBuf::from(manifest_dir);
    loop {
        path.push("Cargo.toml");
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Some(v) = parse_workspace_metadata_version(&content) {
                    return Some(v);
                }
            }
        }
        path.pop();
        path = path.parent()?.to_path_buf();
    }
}

fn parse_workspace_metadata_version(content: &str) -> Option<String> {
    let mut in_section = false;
    for line in content.lines() {
        let line = line.trim();
        if line == "[workspace.metadata.workspaces]" {
            in_section = true;
            continue;
        }
        if in_section {
            if line.starts_with('[') {
                break;
            }
            if let Some(rest) = line.strip_prefix("version") {
                let rest = rest
                    .trim_start()
                    .strip_prefix('=')
                    .map(|s| s.trim_start())?;
                let version = rest.trim_matches(|c| c == '"' || c == '\'');
                if !version.is_empty() {
                    return Some(version.to_string());
                }
            }
        }
    }
    None
}

/// Run a command and return stdout. Fails if the command exits non-zero.
pub fn run_command(
    cmd: &str,
    args: &[&str],
    cwd: Option<&Path>,
) -> Result<String, Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=PATH");
    let output = Command::new(cmd)
        .args(args)
        .current_dir(cwd.unwrap_or(Path::new(".")))
        .output()?;
    if !output.status.success() {
        return Err(format!("{} failed", cmd).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// Return (git_describe, git_commit) for the repo containing `pkg_dir`.
/// Resolves relative `git rev-parse --git-dir` against `pkg_dir` so
/// rerun-if-changed paths are correct and cached builds pick up git changes.
pub fn git_details(pkg_dir: &str) -> Result<(String, String), Box<dyn std::error::Error>> {
    let pkg_path = Path::new(pkg_dir);
    let git_dir_raw = run_command("git", &["rev-parse", "--git-dir"], Some(pkg_path))?;
    let git_dir_trimmed = git_dir_raw.trim();
    let git_dir = if Path::new(git_dir_trimmed).is_absolute() {
        Path::new(git_dir_trimmed).to_path_buf()
    } else {
        pkg_path.join(git_dir_trimmed)
    };

    for subpath in ["HEAD", "logs/HEAD", "index"] {
        if let Ok(p) = git_dir.join(subpath).canonicalize() {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }

    let git_describe = run_command(
        "git",
        &[
            "describe",
            "--always",
            "--dirty=-modified",
            "--tags",
            "--match",
            "[0-9]*",
        ],
        Some(pkg_path),
    )?;
    let git_commit = run_command("git", &["rev-parse", "--short", "HEAD"], Some(pkg_path))?;
    Ok((git_describe, git_commit))
}
