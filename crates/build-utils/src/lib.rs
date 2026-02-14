//! Shared build script utilities for Calimero binaries and crates.
//!
//! Provides workspace version reading and git metadata so build scripts stay DRY
//! and consistent (e.g. correct git_dir resolution for rerun-if-changed).

use std::error::Error;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use toml::Value;

/// Read `[workspace.metadata.workspaces].version` from the workspace root Cargo.toml.
/// Used so binaries and crates get the release version instead of the workspace
/// placeholder `0.0.0`. Returns `None` if not found (e.g. not in a workspace).
pub fn read_workspace_version() -> Option<String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let (version, version_file) = read_workspace_version_for_dir(Path::new(&manifest_dir))?;

    // Re-run build scripts when the resolved workspace version changes.
    println!("cargo:rerun-if-changed={}", version_file.display());
    Some(version)
}

fn read_workspace_version_for_dir(manifest_dir: &Path) -> Option<(String, PathBuf)> {
    let mut dir = manifest_dir.to_path_buf();

    loop {
        let cargo_toml = dir.join("Cargo.toml");

        if cargo_toml.exists() {
            if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                if let Some(v) = parse_workspace_metadata_version(&content) {
                    return Some((v, cargo_toml));
                }
            }
        }

        if !dir.pop() {
            break;
        }
    }

    None
}

fn parse_workspace_metadata_version(content: &str) -> Option<String> {
    let value: Value = toml::from_str(content).ok()?;
    let version = value
        .get("workspace")?
        .get("metadata")?
        .get("workspaces")?
        .get("version")?
        .as_str()?
        .trim();

    (!version.is_empty()).then(|| version.to_string())
}

/// Run a command and return stdout. Fails if the command exits non-zero.
pub fn run_command(cmd: &str, args: &[&str], cwd: Option<&Path>) -> Result<String, Box<dyn Error>> {
    let output = Command::new(cmd)
        .args(args)
        .current_dir(cwd.unwrap_or(Path::new(".")))
        .output()?;

    if !output.status.success() {
        let command_line = if args.is_empty() {
            cmd.to_owned()
        } else {
            format!("{cmd} {}", args.join(" "))
        };

        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();

        if stderr.is_empty() {
            return Err(format!("`{command_line}` failed with status: {}", output.status).into());
        }

        return Err(format!(
            "`{command_line}` failed with status: {} (stderr: {stderr})",
            output.status
        )
        .into());
    }

    Ok(String::from_utf8(output.stdout)?)
}

/// Git metadata for the repo containing a crate/package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitInfo {
    pub describe: String,
    pub commit: String,
}

impl GitInfo {
    fn unknown() -> Self {
        Self {
            describe: "unknown".to_owned(),
            commit: "unknown".to_owned(),
        }
    }
}

/// Return git describe + commit for the repo containing `pkg_dir`.
/// Resolves relative `git rev-parse --git-dir` against `pkg_dir` so
/// rerun-if-changed paths are correct and cached builds pick up git changes.
pub fn git_details(pkg_dir: &str) -> Result<GitInfo, Box<dyn Error>> {
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

    Ok(GitInfo {
        describe: git_describe,
        commit: git_commit,
    })
}

/// Return git metadata, falling back to `unknown` when git info is unavailable.
pub fn git_details_or_unknown(pkg_dir: &str) -> GitInfo {
    match git_details(pkg_dir) {
        Ok(info) => info,
        Err(err) => {
            println!("cargo:warning=unable to determine git version (not in git repository?)");
            println!("cargo:warning={err}");
            GitInfo::unknown()
        }
    }
}

/// Set `<PREFIX>_VERSION`, `<PREFIX>_BUILD`, `<PREFIX>_COMMIT`, `<PREFIX>_RUSTC_VERSION`.
pub fn set_version_env_vars(prefix: &str) -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=PATH");

    let pkg_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let version = read_workspace_version().ok_or_else(|| {
        "failed to read [workspace.metadata.workspaces].version from workspace Cargo.toml"
            .to_owned()
    })?;

    let git_info = git_details_or_unknown(&pkg_dir);

    let rustc_version = rustc_version::version()?.to_string();

    println!("cargo:rustc-env={prefix}_VERSION={version}");
    println!(
        "cargo:rustc-env={prefix}_BUILD={}",
        git_info.describe.trim()
    );
    println!("cargo:rustc-env={prefix}_COMMIT={}", git_info.commit.trim());
    println!("cargo:rustc-env={prefix}_RUSTC_VERSION={rustc_version}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        git_details_or_unknown, parse_workspace_metadata_version, read_workspace_version_for_dir,
        run_command,
    };

    #[test]
    fn parse_workspace_metadata_version_handles_inline_comments() {
        let content = r#"
[workspace.metadata.workspaces]
version = "1.2.3"   # or e.g. "0.10.0-rc.43"
"#;

        assert_eq!(
            parse_workspace_metadata_version(content),
            Some("1.2.3".to_owned())
        );
    }

    #[test]
    fn parse_workspace_metadata_version_ignores_other_version_like_keys() {
        let content = r#"
[workspace.metadata.workspaces]
version_prefix = "v"
version = "2.0.1"
"#;

        assert_eq!(
            parse_workspace_metadata_version(content),
            Some("2.0.1".to_owned())
        );
    }

    #[test]
    fn parse_workspace_metadata_version_returns_none_without_target_field() {
        let content = r#"
[workspace.metadata.workspaces]
exclude = ["./apps/example"]
"#;

        assert_eq!(parse_workspace_metadata_version(content), None);
    }

    #[test]
    fn read_workspace_version_for_dir_finds_workspace_root_file() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");
        let root = tmp.path();
        let nested = root.join("crates/example");
        std::fs::create_dir_all(&nested).expect("nested dir must be creatable");

        std::fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace.metadata.workspaces]
version = "3.4.5"
"#,
        )
        .expect("workspace Cargo.toml must be writable");

        let (version, version_file) =
            read_workspace_version_for_dir(&nested).expect("workspace version should resolve");

        assert_eq!(version, "3.4.5");
        assert_eq!(version_file, root.join("Cargo.toml"));
    }

    #[test]
    fn run_command_includes_stderr_context_on_failure() {
        let err = run_command("git", &["this-command-does-not-exist"], None).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("stderr"));
    }

    #[test]
    fn git_details_or_unknown_returns_unknown_outside_git_repo() {
        let dir = tempfile::tempdir().expect("temp dir must be creatable");
        let git_info = git_details_or_unknown(
            dir.path()
                .to_str()
                .expect("temp dir path should be valid utf-8"),
        );

        assert_eq!(git_info.describe, "unknown");
        assert_eq!(git_info.commit, "unknown");
    }
}
