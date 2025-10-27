use std::path::Path;
use std::process::Command;

use eyre::{bail, eyre, Result as EyreResult};

fn main() {
    if let Err(err) = try_main() {
        eprintln!("build.rs error: {err:?}");
        std::process::exit(1);
    }
}

fn try_main() -> EyreResult<()> {
    let (git_build, git_commit) = git_details()?;

    let rustc_version = rustc_version::version()?.to_string();

    println!("cargo:rustc-env=CALIMERO_BUILD={}", git_build.trim());
    println!("cargo:rustc-env=CALIMERO_COMMIT={}", git_commit.trim());
    println!("cargo:rustc-env=CALIMERO_RUSTC_VERSION={rustc_version}");

    Ok(())
}

fn git_details() -> EyreResult<(String, String)> {
    let pkg_dir = env!("CARGO_MANIFEST_DIR");
    let git_dir = run_command("git", &["rev-parse", "--git-dir"], Some(Path::new(pkg_dir)));
    let git_dir = match &git_dir {
        Ok(dir) => Path::new(dir.trim()),
        Err(msg) => {
            println!("cargo:warning=unable to determine git version (not in git repository?)");
            println!("cargo:warning={msg}");
            return Ok(("unknown".to_owned(), "unknown".to_owned()));
        }
    };

    for subpath in ["HEAD", "logs/HEAD", "index"] {
        let path = git_dir.join(subpath).canonicalize()?;
        println!("cargo:rerun-if-changed={}", path.display());
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
        None,
    )?;

    let git_commit = run_command("git", &["rev-parse", "--short", "HEAD"], None)?;

    Ok((git_describe, git_commit))
}

fn run_command(command: &str, args: &[&str], cwd: Option<&Path>) -> EyreResult<String> {
    println!("cargo:rerun-if-env-changed=PATH");

    let mut cmd = Command::new(command);

    let _ignored = cmd.args(args);

    if let Some(cwd) = cwd {
        let _ignored = cmd.current_dir(cwd);
    }

    let output = cmd
        .output()
        .map_err(|e| eyre!("failed to execute `{}`: {}", command, e))?;

    if !output.status.success() {
        bail!("`{}` failed with status: {}", command, output.status);
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| eyre!("invalid UTF-8 output from `{}`: {}", command, e))?;

    Ok(stdout)
}
