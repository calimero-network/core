//! `mero-app-build codegen` and `mero-app-build abi-diff`: an app's OpenAPI document and typed
//! clients from its ABI, and what changed between two of its ABIs.

use std::fs;
use std::path::{Path, PathBuf};

use calimero_app_abi::codegen::{python, typescript, Target};
use calimero_app_abi::diff::diff;
use calimero_app_abi::load::{load_one, load_path, Loaded};
use calimero_app_abi::openapi::{document, Info};
use eyre::{bail, eyre, Result as EyreResult};

pub const CODEGEN_USAGE: &str = "usage: mero-app-build codegen --abi <abi.json|app.wasm|app.mpk|manifest.json> --lang <ts|python|openapi> [-o <dir>] [--name <name>] [--service <name>]";
pub const DIFF_USAGE: &str =
    "usage: mero-app-build abi-diff <old> <new> [--service <name>] [--json]";

/// What `abi-diff` found: the caller exits non-zero on a breaking change.
pub enum DiffOutcome {
    Compatible,
    Breaking,
}

#[derive(Clone, Copy)]
enum Lang {
    TypeScript,
    Python,
    OpenApi,
}

pub fn codegen(args: impl Iterator<Item = String>) -> EyreResult<()> {
    let mut abi = None;
    let mut lang = None;
    let mut out = PathBuf::from(".");
    let mut name = None;
    let mut service = None;
    let mut args = args;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| eyre!(CODEGEN_USAGE))?;
        match flag.as_str() {
            "--abi" => abi = Some(PathBuf::from(value)),
            "--lang" => {
                lang = Some(match value.as_str() {
                    "ts" | "typescript" => Lang::TypeScript,
                    "python" | "py" => Lang::Python,
                    "openapi" => Lang::OpenApi,
                    other => bail!("unknown --lang {other:?}; {CODEGEN_USAGE}"),
                });
            }
            "-o" | "--out" => out = PathBuf::from(value),
            "--name" => name = Some(value),
            "--service" => service = Some(value),
            _ => bail!(CODEGEN_USAGE),
        }
    }
    let (Some(abi), Some(lang)) = (abi, lang) else {
        bail!(CODEGEN_USAGE);
    };
    let all = match &service {
        Some(s) => vec![load_one(&abi, Some(s))?],
        None => load_path(&abi)?,
    };
    let several = all.len() > 1;
    for loaded in &all {
        let dir = match (&loaded.service, several) {
            (Some(s), true) => out.join(s),
            _ => out.clone(),
        };
        let name = name
            .clone()
            .or_else(|| loaded.service.clone())
            .or_else(|| loaded.package.as_deref().map(last_segment))
            .unwrap_or_else(|| stem(&abi));
        write_outputs(loaded, lang, &dir, &name)?;
    }
    Ok(())
}

fn write_outputs(loaded: &Loaded, lang: Lang, dir: &Path, name: &str) -> EyreResult<()> {
    fs::create_dir_all(dir)?;
    let info = Info {
        title: loaded.package.clone().unwrap_or_else(|| name.to_owned()),
        version: loaded
            .app_version
            .clone()
            .unwrap_or_else(|| "0.0.0".to_owned()),
        app: None,
    };
    let doc = document(&loaded.abi, &info);
    write(
        &dir.join("openapi.json"),
        &format!("{}\n", serde_json::to_string_pretty(&doc)?),
    )?;
    let target = Target { name };
    match lang {
        Lang::OpenApi => {}
        Lang::TypeScript => write(&dir.join("client.ts"), &typescript(&loaded.abi, &target))?,
        Lang::Python => write(&dir.join("client.py"), &python(&loaded.abi, &target))?,
    }
    Ok(())
}

fn write(path: &Path, text: &str) -> EyreResult<()> {
    fs::write(path, text)?;
    println!("wrote {}", path.display());
    Ok(())
}

/// `com.example.kv-store` → `kv-store`.
fn last_segment(package: &str) -> String {
    package.rsplit('.').next().unwrap_or(package).to_owned()
}

fn stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    // `kv.abi.json`, `app.component.wasm`: the first part names the app.
    stem.split('.').next().unwrap_or_default().to_owned()
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
