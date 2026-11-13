//! `mero-app-build codegen` and `mero-app-build abi-diff`: an app's OpenAPI document and typed
//! clients from its ABI, and what changed between two of its ABIs.

use std::path::PathBuf;

use crate::codegen::{Codegen, Lang};
use calimero_app_abi::diff::diff;
use calimero_app_abi::load::load_one;
use eyre::{bail, eyre, Result as EyreResult};

pub const CODEGEN_USAGE: &str = "usage: meroctl app codegen --abi <abi.json|app.wasm|app.mpk|manifest.json> --lang <ts|python|swift|kotlin|openapi> [-o <dir>] [--name <name>] [--package <kotlin package>] [--kotlin-serialization-plugin] [--service <name>] [--no-openapi]";
pub const DIFF_USAGE: &str = "usage: meroctl app abi diff <old> <new> [--service <name>] [--json]";

/// What `abi-diff` found: the caller exits non-zero on a breaking change.
pub enum DiffOutcome {
    Compatible,
    Breaking,
}

pub fn codegen(args: impl Iterator<Item = String>) -> EyreResult<()> {
    let mut abi = None;
    let mut lang = None;
    let mut out = PathBuf::from(".");
    let mut name = None;
    let mut service = None;
    let mut package = None;
    let mut plugin = false;
    let mut openapi = true;
    let mut args = args;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "-h" | "--help" => {
                println!("{CODEGEN_USAGE}");
                return Ok(());
            }
            "--no-openapi" => {
                openapi = false;
                continue;
            }
            "--kotlin-serialization-plugin" => {
                plugin = true;
                continue;
            }
            _ => {}
        }
        let value = args.next().ok_or_else(|| eyre!(CODEGEN_USAGE))?;
        match flag.as_str() {
            "--abi" => abi = Some(PathBuf::from(value)),
            "--lang" => {
                lang = Some(
                    value
                        .parse::<Lang>()
                        .map_err(|e| eyre!("{e}; {CODEGEN_USAGE}"))?,
                );
            }
            "-o" | "--out" => out = PathBuf::from(value),
            "--name" => name = Some(value),
            "--service" => service = Some(value),
            "--package" => package = Some(value),
            _ => bail!(CODEGEN_USAGE),
        }
    }
    let (Some(abi), Some(lang)) = (abi, lang) else {
        bail!(CODEGEN_USAGE);
    };
    let mut job = Codegen::new(abi, lang, out);
    job.name = name;
    job.service = service;
    job.package = package;
    job.kotlin_serialization_plugin = plugin;
    job.openapi = openapi;
    for path in job.run()? {
        println!("wrote {}", path.display());
    }
    Ok(())
}

pub fn abi_diff(args: impl Iterator<Item = String>) -> EyreResult<DiffOutcome> {
    let mut paths = Vec::new();
    let mut service = None;
    let mut as_json = false;
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--service" => service = Some(args.next().ok_or_else(|| eyre!(DIFF_USAGE))?),
            "--json" => as_json = true,
            "-h" | "--help" => {
                println!("{DIFF_USAGE}");
                return Ok(DiffOutcome::Compatible);
            }
            _ if arg.starts_with("--") => bail!(DIFF_USAGE),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    let [old, new] = paths.as_slice() else {
        bail!(DIFF_USAGE);
    };
    let old = load_one(old, service.as_deref())?;
    let new = load_one(new, service.as_deref())?;
    let report = diff(&old.abi, &new.abi);
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    } else {
        print!("{report}");
    }
    Ok(if report.is_breaking() || report.state_error.is_some() {
        DiffOutcome::Breaking
    } else {
        DiffOutcome::Compatible
    })
}
