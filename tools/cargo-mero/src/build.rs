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
    if !args.features.is_empty() {
        let _ = build_cmd.arg("--features");
        let _ = build_cmd.arg(args.features.join(","));
    }
    if args.no_default_features {
        let _ = build_cmd.arg("--no-default-features");
    }

    if let Some(package) = args.package {
        let _ = build_cmd.arg("--package");
        let _ = build_cmd.arg(package);
    }

    let mut child = build_cmd.stdout(Stdio::piped()).spawn()?;

    let child_stdout = child
        .stdout
        .take()
        .wrap_err("could not attach to child stdout")?;

    // Extract wasm path from cargo build output
    let mut artifact = None;
    let stdout_reader = std::io::BufReader::new(child_stdout);
    for message in Message::parse_stream(stdout_reader) {
        match message? {
            Message::CompilerArtifact(a) => {
                let _old = artifact.replace(a);
            }
            _ => {}
        }
    }

    let artifact = artifact.as_ref().unwrap();
    let manifest_dir_path = artifact.manifest_path.parent().unwrap();
    let wasm_path = &artifact.filenames[0];
    let wasm_file = wasm_path.file_name().unwrap();

    let output = child.wait().wrap_err("cargo build failed")?;

    if !output.success() {
        bail!("cargo build command failed");
    }

    // Copy wasm to res folder
    let res_path = Path::new(manifest_dir_path).join("res");
    if !res_path.exists() {
        fs::create_dir(&res_path)?;
    }
    let _ = fs::copy(wasm_path, res_path.join(&wasm_file))?;

    // Optimize wasm if wasm-opt is present
    if wasm_opt_installed().await {
        println!("⚙️ \x1b[1;32mOptimizing\x1b[0m...");

        let output = Command::new("wasm-opt")
            .arg("-Oz")
            .arg(res_path.join(&wasm_file))
            .arg("-o")
            .arg(res_path.join(&wasm_file))
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
