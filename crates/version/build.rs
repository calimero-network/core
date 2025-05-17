use std::env;
use std::process::Command;

use eyre::{eyre, Result, WrapErr};

fn main() -> Result<()> {
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
    println!("cargo:rerun-if-env-changed=PATH");

    let git_commit_hash = get_output(["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| {
        println!("cargo:warning=Failed to get git commit hash, defaulting to 'unknown'");
        "unknown".to_string()
    });
    println!("cargo:rustc-env=GIT_COMMIT_HASH={}", git_commit_hash);

    let git_describe =
        get_output(["describe", "--tags", "--dirty", "--always"]).unwrap_or_else(|| {
            println!("cargo:warning=Failed to get git describe, defaulting to 'unknown'");
            "unknown".to_string()
        });
    println!("cargo:rustc-env=GIT_DESCRIBE={}", git_describe);

    let rustc_version = Command::new("rustc")
        .arg("--version")
        .output()
        .wrap_err("Failed to execute rustc --version")?
        .stdout;
    let rustc_version_str =
        String::from_utf8(rustc_version).wrap_err("rustc output was not valid UTF-8")?;
    println!("cargo:rustc-env=RUSTC_VERSION={}", rustc_version_str.trim());

    Ok(())
}

fn get_output(args: impl IntoIterator<Item = &'static str>) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}
