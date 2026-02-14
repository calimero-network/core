// Set version and build metadata env vars for the binary (NEAR-style).
// See https://github.com/near/nearcore/blob/master/neard/src/main.rs

use std::path::Path;
use std::process::Command;

fn main() {
    let (build, commit) = git_details().unwrap_or(("unknown".into(), "unknown".into()));
    let rustc_version = rustc_version::version()
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "unknown".into());

    println!("cargo:rustc-env=MEROCTL_VERSION={}", env!("CARGO_PKG_VERSION"));
    println!("cargo:rustc-env=MEROCTL_BUILD={}", build.trim());
    println!("cargo:rustc-env=MEROCTL_COMMIT={}", commit.trim());
    println!("cargo:rustc-env=MEROCTL_RUSTC_VERSION={}", rustc_version);
}

fn git_details() -> Result<(String, String), Box<dyn std::error::Error>> {
    let pkg_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let git_dir = run_command("git", &["rev-parse", "--git-dir"], Some(Path::new(&pkg_dir)))?;
    let git_dir = Path::new(git_dir.trim());

    for subpath in ["HEAD", "logs/HEAD", "index"] {
        if let Ok(p) = git_dir.join(subpath).canonicalize() {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }

    let git_describe = run_command(
        "git",
        &["describe", "--always", "--dirty=-modified", "--tags", "--match", "[0-9]*"],
        None,
    )?;
    let git_commit = run_command("git", &["rev-parse", "--short", "HEAD"], None)?;
    Ok((git_describe, git_commit))
}

fn run_command(cmd: &str, args: &[&str], cwd: Option<&Path>) -> Result<String, Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=PATH");
    let output = Command::new(cmd).args(args).current_dir(cwd.unwrap_or(Path::new("."))).output()?;
    if !output.status.success() {
        return Err(format!("{} failed", cmd).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}
