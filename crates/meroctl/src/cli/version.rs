use std::process::Command;
use std::sync::OnceLock;

static VERSION_INFO: OnceLock<String> = OnceLock::new();

pub fn get_version_info() -> &'static str {
    VERSION_INFO.get_or_init(|| {
        // Get git revision
        let git_revision = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .unwrap_or_else(|| "unknown".to_string());

        // Check if working directory is clean
        let is_modified = Command::new("git")
            .args(["diff", "--quiet"])
            .status()
            .map(|status| !status.success())
            .unwrap_or(false);

        // Get rustc version (extract only version number)
        let rustc_version = Command::new("rustc")
            .arg("--version")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|s| s.split_whitespace().nth(1).unwrap_or("unknown").to_string())
            .unwrap_or_else(|| "unknown".to_string());

        format!(
            "(release trunk) (build {}-{}{}) (rustc {}) (protocol {})",
            env!("CARGO_PKG_VERSION"),
            git_revision.trim(),
            if is_modified { "-modified" } else { "" },
            rustc_version.trim(),
            env!("CARGO_PKG_VERSION").split('.').next().unwrap_or("0")
        )
    })
} 