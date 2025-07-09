use std::fs;
use std::path::Path;
use std::process::Stdio;

use cargo_metadata::MetadataCommand;
use eyre::{bail, Context};
use tokio::process::Command;

use crate::cli::BuildOpts;

pub async fn run(args: BuildOpts) -> eyre::Result<()> {
    println!("🏗️ \x1b[1;32mBuilding\x1b[0m...");
    let output = Command::new("rustup")
        .arg("target")
        .arg("add")
        .arg("wasm32-unknown-unknown")
        .output()
        .await
        .wrap_err("Adding wasm32-unknown-unknown target failed")?;

    if !output.status.success() {
        bail!("Adding wasm32-unknown-unknown target command failed");
    }

    let mut build_cmd = Command::new("cargo");
    let _ = build_cmd
        .arg("build")
        .arg("--target")
        .arg("wasm32-unknown-unknown");

    // Add additional pass-through cargo arguments
    if !args.no_locked {
        let _ = build_cmd.arg("--locked");
    }
    if !args.no_release {
        let _ = build_cmd.arg("--profile");
        let _ = build_cmd.arg("app-release");
    }
    if args.verbose {
        let _ = build_cmd.arg("--verbose");
    }
    if args.quiet {
        let _ = build_cmd.arg("--quiet");
    }
    if let Some(features) = args.features {
        let _ = build_cmd.arg("--features");
        let _ = build_cmd.arg(features);
    }
    if args.no_default_features {
        let _ = build_cmd.arg("--no-default-features");
    }

    let output = build_cmd
        .spawn()?
        .wait()
        .await
        .wrap_err("cargo build failed")?;

    if !output.success() {
        bail!("cargo build command failed");
    }

    // Copy wasm to res folder
    if !Path::new("res/").exists() {
        fs::create_dir("res")?;
    }

    let package_name = MetadataCommand::new().exec()?.packages[0]
        .name
        .clone()
        .into_inner()
        .replace("-", "_");
    let wasm_file = format!("{}.wasm", package_name);
    let wasm_path = Path::new("./target/wasm32-unknown-unknown/app-release/").join(&wasm_file);

    let _ = fs::copy(wasm_path, Path::new("./res/").join(&wasm_file))?;

    // Optimize wasm if wasm-opt is present
    if wasm_opt_installed().await {
        println!("⚙️ \x1b[1;32mOptimizing\x1b[0m...");

        let output = Command::new("wasm-opt")
            .arg("-Oz")
            .arg(Path::new("./res/").join(&wasm_file))
            .arg("-o")
            .arg(Path::new("./res/").join(&wasm_file))
            .output()
            .await?;

        if !output.status.success() {
            bail!(
                "wasm optimization command failed -> code: {:?},\n stderr: {:#?} \n stdout: {:?} \n",
                output.status.code(),
                String::from_utf8(output.stderr),
                output.stdout
            );
        }
        println!("✅ \x1b[1;32mOptimization complete\x1b[0m");
    } else {
        println!("wasm-opt not found, skipping optimization");
    }
    Ok(())
}

async fn wasm_opt_installed() -> bool {
    Command::new("wasm-opt")
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .await
        .is_ok()
}
