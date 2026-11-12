use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use calimero_app_build::build_components;
use eyre::Result as EyreResult;

mod abi_tools;

use abi_tools::DiffOutcome;

const COMPUTE_FILE: &str = "compute.component.wasm"; // beside the app component unless --compute-out says otherwise

fn write(out: &Path, bytes: &[u8]) -> EyreResult<()> {
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, bytes)?;
    println!("wrote {} ({} bytes)", out.display(), bytes.len());
    Ok(())
}

fn main() -> EyreResult<ExitCode> {
    let mut args = env::args().skip(1);
    let usage = "usage: mero-app-build <crate-dir> [--out <file>] [--compute-out <file>] [--target-dir <dir>]\n       mero-app-build codegen ...\n       mero-app-build abi-diff ...";
    let first = args.next().ok_or_else(|| eyre::eyre!(usage))?;
    // A crate directory named like a subcommand is passed as `./codegen`.
    match first.as_str() {
        "codegen" => {
            abi_tools::codegen(args)?;
            return Ok(ExitCode::SUCCESS);
        }
        "abi-diff" => {
            return Ok(match abi_tools::abi_diff(args)? {
                DiffOutcome::Compatible => ExitCode::SUCCESS,
                DiffOutcome::Breaking => ExitCode::from(1),
            });
        }
        _ => {}
    }
    let crate_dir = PathBuf::from(first);
    let mut out = None;
    let mut compute_out = None;
    let mut target = None;
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| eyre::eyre!(usage))?;
        match flag.as_str() {
            "--out" => out = Some(value),
            "--compute-out" => compute_out = Some(value),
            "--target-dir" => target = Some(value),
            _ => eyre::bail!(usage),
        }
    }
    let out = out.unwrap_or_else(|| crate_dir.join("app.component.wasm"));
    let target = target.unwrap_or_else(|| crate_dir.join("target"));
    let built = build_components(&crate_dir, &target)?;
    write(&out, &built.app.component)?;
    match (&built.compute, compute_out) {
        (Some(compute), path) => {
            let path = path.unwrap_or_else(|| out.with_file_name(COMPUTE_FILE));
            write(&path, &compute.component)?;
        }
        (None, Some(_)) => eyre::bail!("{} has no compute methods", crate_dir.display()),
        (None, None) => {}
    }
    Ok(ExitCode::SUCCESS)
}
