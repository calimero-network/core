use std::fs;
use std::path::PathBuf;
use std::process::Stdio;

use cargo_metadata::MetadataCommand;
use eyre::{bail, Context};

use crate::utils::new_command;

pub async fn run(args: Vec<String>) -> eyre::Result<()> {
    let output = new_command("rustup")
        .arg("target")
        .arg("add")
        .arg("wasm32-unknown-unknown")
        .output()
        .await
        .wrap_err("Adding wasm32-unknown-unknown target failed")?;

    if !output.status.success() {
        bail!("Adding wasm32-unknown-unknown target command failed");
    }

    let mut build_cmd = new_command("cargo");
    let _ = build_cmd
        .arg("build")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .arg("--profile")
        .arg("app-release");

    // Add additional pass-through cargo arguments
    for arg in args {
        let _ = build_cmd.arg(arg);
    }

    let output = build_cmd.output().await.wrap_err("cargo build failed")?;

    if !output.status.success() {
        bail!("cargo build command failed");
    }

    // Copy wasm to res folder
    fs::create_dir("res")?;

    let package_name = MetadataCommand::new().exec()?.packages[0]
        .name
        .clone()
        .into_inner()
        .replace("-", "_");
    let wasm_file = format!("{}.wasm", package_name);
    let wasm_path = build_path("./target/wasm32-unknown-unknown/app-release/", &wasm_file);

    let _ = fs::copy(wasm_path, build_path("./res/", &wasm_file))?;

    // Optimize wasm if wasm-opt is present
    if wasm_opt_installed().await {
        println!("wasm-opt found, optimizing...");

        let output = new_command("wasm-opt")
            .arg("-Oz")
            .arg(build_path("./res/", &wasm_file))
            .arg("-o")
            .arg(build_path("./res/", &wasm_file))
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
        println!("wasm optimization complete");
    } else {
        println!("wasm-opt not found, skipping optimization");
    }
    Ok(())
}

async fn wasm_opt_installed() -> bool {
    new_command("wasm-opt")
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .await
        .is_ok()
}

/// Builds a path from two `&str`s
fn build_path(a: &str, b: &str) -> PathBuf {
    let mut path = PathBuf::from(a);
    path.push(b);
    path
}
