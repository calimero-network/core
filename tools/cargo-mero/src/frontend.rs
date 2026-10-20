//! A bundle's web frontend: checking one inside an `.mpk` and reporting the
//! result for humans or, with `--format json`, for CI. The rules themselves
//! live in `calimero_bundle`, so the CLI and the node cannot disagree.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use calimero_bundle::{
    check_frontend_html, verify_frontend_file, BundleFrontend, BundleManifest, FrontendError,
    MAX_FRONTEND_BYTES,
};
use camino::Utf8Path;
use eyre::{bail, eyre, Context, Result};
use flate2::read::GzDecoder;
use serde::Serialize;

use crate::manifest;
use crate::OutputFormat;

/// Reported for any failure that is not a `FrontendError`.
const GENERIC_ERROR_CODE: &str = "BUNDLE_ERROR";
const ARCHIVE_ROOT: &str = "frontend"; // where the frontend sits in the bundle
const ENTRY: &str = "frontend/index.html";
const SOURCE_MAP_SUFFIX: &str = ".map"; // never shipped: it only helps a debugger

/// What `--format json` prints on success.
#[derive(Serialize)]
pub struct Summary {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontend: Option<FrontendSummary>,
}

#[derive(Serialize)]
pub struct FrontendSummary {
    pub entry: String,
    pub files: usize,
    pub bytes: u64,
}

impl FrontendSummary {
    pub fn of(frontend: &BundleFrontend) -> Self {
        Self {
            entry: frontend.entry.clone(),
            files: frontend.files.len(),
            bytes: frontend.files.iter().map(|f| f.size).sum(),
        }
    }
}

/// A built frontend directory, validated, with the exact bytes each entry in
/// `frontend.files` was hashed from; packaging writes these, never a re-read.
pub struct Collected {
    pub frontend: BundleFrontend,
    pub contents: Vec<Vec<u8>>,
}

/// Walk a build directory and apply the node's frontend rules to it, so a bad
/// build fails here, before the wasm build, with the code the node would use.
pub fn collect(dir: &Path) -> Result<Collected> {
    let mut found = Vec::new();
    walk(dir, ARCHIVE_ROOT, &mut found)?;
    found.sort_by(|a, b| a.1.cmp(&b.1));

    let mut files = Vec::with_capacity(found.len());
    let mut contents = Vec::with_capacity(found.len());
    for (source, rel) in found {
        let bytes =
            fs::read(&source).wrap_err_with(|| format!("failed to read {}", source.display()))?;
        files.push(manifest::artifact_from_bytes(rel, &bytes));
        contents.push(bytes);
    }

    let frontend = BundleFrontend {
        entry: ENTRY.to_owned(),
        files,
    };
    frontend.validate()?;
    for (file, bytes) in frontend.files.iter().zip(&contents) {
        verify_frontend_file(file, bytes)?;
        if is_html(&file.path) {
            check_frontend_html(&file.path, bytes)?;
        }
    }
    Ok(Collected { frontend, contents })
}

fn is_html(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("html"))
}

/// Every regular file under `dir` as (source, archive path). A symlink could
/// point outside the build, so it is refused rather than followed.
fn walk(dir: &Path, rel: &str, out: &mut Vec<(PathBuf, String)>) -> Result<()> {
    for entry in fs::read_dir(dir).wrap_err_with(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let rel = format!("{rel}/{}", name.to_string_lossy());
        let kind = entry.file_type()?;
        if kind.is_symlink() || name.to_str().is_none() {
            return Err(FrontendError::UnsafePath(rel).into());
        }
        if kind.is_dir() {
            walk(&entry.path(), &rel, out)?;
        } else if !rel.ends_with(SOURCE_MAP_SUFFIX) {
            out.push((entry.path(), rel));
        }
    }
    Ok(())
}

/// Open an `.mpk` and run the node's frontend checks over its manifest and every file's bytes.
pub fn check_mpk(mpk: &Utf8Path) -> Result<FrontendSummary> {
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
        if is_html(&file.path) {
            check_frontend_html(&file.path, bytes)?;
        }
    }
    Ok(FrontendSummary::of(&frontend))
}

/// `--check-frontend`: print the verdict in the chosen format.
pub fn run_check(mpk: &Utf8Path, format: OutputFormat) -> Result<()> {
    let result = check_mpk(mpk);
    if let (OutputFormat::Human, Ok(summary)) = (format, &result) {
        println!(
            "frontend ok: {} files, {} bytes",
            summary.files, summary.bytes
        );
    }
    report(
        format,
        result.map(|frontend| Summary {
            ok: true,
            output: None,
            frontend: Some(frontend),
        }),
    )
}

/// In JSON mode print the outcome as one object on stdout; a failure then exits
/// non-zero here, instead of returning. Human mode leaves printing to the caller.
pub fn report(format: OutputFormat, result: Result<Summary>) -> Result<()> {
    match (format, result) {
        (OutputFormat::Human, result) => result.map(|_| ()),
        (OutputFormat::Json, Ok(summary)) => {
            println!("{}", serde_json::to_string(&summary)?);
            Ok(())
        }
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
        | FrontendError::NoEntry(path)
        | FrontendError::UnsupportedType(path)
        | FrontendError::HashMismatch(path)
        | FrontendError::InvalidHtml(path)
        | FrontendError::Base(path)
        | FrontendError::EntryNotHtml(path)
        | FrontendError::MissingFile(path)
        | FrontendError::SizeMismatch { path, .. }
        | FrontendError::RemoteResource { path, .. }
        | FrontendError::EscapingUrl { path, .. }
        | FrontendError::InlineScript { path, .. } => Some(path),
        FrontendError::TooLarge => None,
    }
}

fn hint(err: &FrontendError) -> &'static str {
    match err {
        FrontendError::UnsafePath(_) => {
            "use plain files with names of letters, digits and . _ ~ @ + -; symlinks are not allowed"
        }
        FrontendError::DuplicatePath(_) => "list each file once, ignoring letter case",
        FrontendError::NoEntry(_) => "the build must contain an index.html at its top level",
        FrontendError::EntryNotHtml(_) => "the entry must be an .html file",
        FrontendError::UnsupportedType(_) => {
            "remove the file or use a served type (html, js, css, json, images, fonts, wasm)"
        }
        FrontendError::TooLarge => "shrink the build; the cap covers every file's size together",
        FrontendError::SizeMismatch { .. } | FrontendError::HashMismatch(_) => {
            "the file changed after the manifest was written; rebuild the bundle"
        }
        FrontendError::MissingFile(_) => "rebuild the bundle; the archive lacks a file it lists",
        FrontendError::RemoteResource { .. } => {
            "bundle the file and load it from a relative path instead of a remote URL"
        }
        FrontendError::EscapingUrl { .. } | FrontendError::Base(_) => {
            "build with relative URLs (Vite `base: './'`) and no <base> element"
        }
        FrontendError::InlineScript { .. } => "move inline scripts and handlers into a script file",
        FrontendError::InvalidHtml(_) => "save the HTML file as UTF-8",
    }
}
