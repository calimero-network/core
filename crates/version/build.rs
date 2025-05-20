use std::process::Command;
use eyre::{eyre, Result};
use rustc_version::version;

fn main() {
    if let Err(err) = try_main() {
        eprintln!("build.rs error: {err}");
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    let git_describe = run_command("git", &[
        "describe", "--always", "--dirty=-modified", "--tags", "--match", "[0-9]*"
    ])?;

    let git_commit = run_command("git", &["rev-parse", "--short", "HEAD"])?;

    let rustc_version = version()
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=CALIMERO_BUILD={}", git_describe.trim());
    println!("cargo:rustc-env=CALIMERO_COMMIT={}", git_commit.trim());
    println!("cargo:rustc-env=CALIMERO_RUSTC_VERSION={}", rustc_version.trim());

    Ok(())
}

fn run_command(command: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(command)
        .args(args)
        .output()
        .map_err(|e| eyre!("failed to execute `{}`: {}", command, e))?;

    if !output.status.success() {
        return Err(eyre!(
            "`{}` failed with status: {}",
            command,
            output.status
        ));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| eyre!("invalid UTF-8 output from `{}`: {}", command, e))?;

    Ok(stdout)
}
