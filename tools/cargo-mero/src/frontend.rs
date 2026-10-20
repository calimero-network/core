//! A bundle's web frontend: checking one inside an `.mpk` and reporting the
//! result for humans or, with `--format json`, for CI. The rules themselves
//! live in `calimero_bundle`, so the CLI and the node cannot disagree.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;

use calimero_bundle::{
    verify_frontend_file, BundleFrontend, BundleManifest, FrontendError, MAX_FRONTEND_BYTES,
};
use camino::Utf8Path;
use eyre::{bail, eyre, Context, Result};
use flate2::read::GzDecoder;
use serde::Serialize;

use crate::OutputFormat;

/// Reported for any failure that is not a `FrontendError`.
const GENERIC_ERROR_CODE: &str = "BUNDLE_ERROR";

#[derive(Serialize)]
pub struct Summary {
    pub ok: bool,
    pub entry: String,
    pub files: usize,
    pub bytes: u64,
}

impl Summary {
    fn of(frontend: &BundleFrontend) -> Self {
        Self {
            ok: true,
            entry: frontend.entry.clone(),
            files: frontend.files.len(),
            bytes: frontend.files.iter().map(|f| f.size).sum(),
        }
    }
}

/// Open an `.mpk` and run the node's frontend checks over its manifest and every file's bytes.
pub fn check_mpk(mpk: &Utf8Path) -> Result<Summary> {
    let file = File::open(mpk).wrap_err_with(|| format!("failed to open {mpk}"))?;
    let mut archive = tar::Archive::new(GzDecoder::new(file));

    let mut manifest = None;
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    for entry in archive
        .entries()
        .wrap_err("failed to read the bundle archive")?
    {
        let entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        let is_manifest = path == "manifest.json";
        if !is_manifest && !path.starts_with("frontend/") {
            continue;
        }
        // One byte past the cap, so an oversized file fails its size check.
        let mut bytes = Vec::new();
        entry.take(MAX_FRONTEND_BYTES + 1).read_to_end(&mut bytes)?;
        if is_manifest {
            manifest = Some(bytes);
        } else {
            files.insert(path, bytes);
        }
    }

    let manifest = manifest.ok_or_else(|| eyre!("{mpk} has no manifest.json"))?;
    let manifest: BundleManifest =
        serde_json::from_slice(&manifest).wrap_err("manifest.json is not a bundle manifest")?;
    let Some(frontend) = manifest.frontend else {
        bail!("the manifest declares no frontend");
    };

    frontend.validate()?;
    for file in &frontend.files {
        let bytes = files
            .get(&file.path)
            .ok_or_else(|| eyre!("'{}' is in the manifest but not in the archive", file.path))?;
        verify_frontend_file(file, bytes)?;
    }
    Ok(Summary::of(&frontend))
}

/// Print a command's outcome in the chosen format. In JSON mode a failure goes
/// to stdout as an object and exits non-zero here, instead of returning.
pub fn report(format: OutputFormat, result: Result<Summary>) -> Result<()> {
    match (format, result) {
        (OutputFormat::Human, Ok(summary)) => {
            println!(
                "frontend ok: {} files, {} bytes",
                summary.files, summary.bytes
            );
            Ok(())
        }
        (OutputFormat::Json, Ok(summary)) => {
            println!("{}", serde_json::to_string(&summary)?);
            Ok(())
        }
        (OutputFormat::Human, Err(err)) => Err(err),
        (OutputFormat::Json, Err(err)) => {
            println!("{}", error_json(&err));
            std::process::exit(1);
        }
    }
}

fn error_json(err: &eyre::Report) -> serde_json::Value {
    match err.downcast_ref::<FrontendError>() {
        Some(frontend) => serde_json::json!({
            "code": frontend.code(),
            "message": frontend.to_string(),
            "path": error_path(frontend),
            "hint": hint(frontend),
        }),
        None => serde_json::json!({
            "code": GENERIC_ERROR_CODE,
            "message": format!("{err:#}"),
            "path": null,
            "hint": null,
        }),
    }
}

fn error_path(err: &FrontendError) -> Option<&str> {
    match err {
        FrontendError::UnsafePath(path)
        | FrontendError::DuplicatePath(path)
        | FrontendError::UnsupportedType(path)
        | FrontendError::HashMismatch(path)
        | FrontendError::InvalidHtml(path) => Some(path),
        FrontendError::SizeMismatch { path, .. } | FrontendError::RemoteScript { path, .. } => {
            Some(path)
        }
        FrontendError::NoEntry(_) | FrontendError::TooLarge => None,
    }
}

fn hint(err: &FrontendError) -> &'static str {
    match err {
        FrontendError::UnsafePath(_) => {
            "ship plain files only: no symlinks, no `..`, no absolute or backslash paths"
        }
        FrontendError::DuplicatePath(_) => "list each file once",
        FrontendError::NoEntry(_) => "the build must contain an index.html at its top level",
        FrontendError::UnsupportedType(_) => {
            "remove the file or use a served type (html, js, css, json, images, fonts, wasm)"
        }
        FrontendError::TooLarge => "shrink the build; the cap covers every file's size together",
        FrontendError::SizeMismatch { .. } | FrontendError::HashMismatch(_) => {
            "the file changed after the manifest was written; rebuild the bundle"
        }
        FrontendError::RemoteScript { .. } => {
            "bundle the script and load it from a relative path instead of a remote URL"
        }
        FrontendError::InvalidHtml(_) => "save the HTML file as UTF-8",
    }
}
