use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use cargo_metadata::Message;
use eyre::{bail, Context, ContextCompat};

use crate::cli::BuildOpts;

pub async fn run(args: BuildOpts) -> eyre::Result<()> {
    println!("🏗️ \x1b[1;32mBuilding\x1b[0m...");
    let output = Command::new("rustup")
        .arg("target")
        .arg("add")
        .arg("wasm32-unknown-unknown")
        .output()
        .wrap_err("Adding wasm32-unknown-unknown target failed")?;

    if !output.status.success() {
        bail!("Adding wasm32-unknown-unknown target command failed");
    }

    let mut build_cmd = Command::new("cargo");
    let _ = build_cmd
        .arg("build")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .arg("--message-format=json-render-diagnostics");

    // Add additional pass-through cargo arguments
    if args.locked {
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

    let mut child = build_cmd.stdout(Stdio::piped()).spawn()?;

    let child_stdout = child
        .stdout
        .take()
        .wrap_err("could not attach to child stdout")?;

    // Extract wasm path from cargo build output
    let mut artifacts = vec![];
    let stdout_reader = std::io::BufReader::new(child_stdout);
    for message in Message::parse_stream(stdout_reader) {
        match message? {
            Message::CompilerArtifact(artifact) => {
                artifacts.push(artifact);
            }
            _ => {}
        }
    }

    let wasm_path = artifacts.last().unwrap().filenames[0].clone();
    let wasm_path_str = wasm_path.clone().into_string();
    let wasm_file = wasm_path_str.split("/").last().unwrap();

    let output = child.wait().wrap_err("cargo build failed")?;

    if !output.success() {
        bail!("cargo build command failed");
    }

    // Copy wasm to res folder
    if !Path::new("res/").exists() {
        fs::create_dir("res")?;
    }
    let _ = fs::copy(wasm_path, Path::new("./res/").join(&wasm_file))?;

    // Optimize wasm if wasm-opt is present
    if wasm_opt_installed().await {
        println!("⚙️ \x1b[1;32mOptimizing\x1b[0m...");

        let output = Command::new("wasm-opt")
            .arg("-Oz")
            .arg(Path::new("./res/").join(&wasm_file))
            .arg("-o")
            .arg(Path::new("./res/").join(&wasm_file))
            .output()?;

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
        .is_ok()
}
