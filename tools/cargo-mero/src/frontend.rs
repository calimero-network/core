//! Checking a bundle's web frontend and reporting it for humans or, with `--format json`, CI.
//! Paths, sizes, hashes and types are `calimero_bundle`'s, shared with the node.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use calimero_bundle::{
    check_frontend_html, mime_type, verify_frontend_file, verify_manifest_signature,
    BundleArtifact, BundleFrontend, BundleManifest, FrontendError, FRONTEND_ROOT,
    MAX_FRONTEND_BYTES, MAX_MANIFEST_BYTES,
};
use camino::Utf8Path;
use eyre::{bail, eyre, Context, Result};
use flate2::read::GzDecoder;
use serde::Serialize;

use crate::manifest;
use crate::OutputFormat;

const GENERIC_ERROR_CODE: &str = "BUNDLE_ERROR"; // any failure that is not a `FrontendError`
const MANIFEST: &str = "manifest.json"; // the signed manifest's archive path
const ENTRY: &str = "frontend/index.html"; // the page a node serves at the app root
const SOURCE_MAP_SUFFIX: &str = ".map"; // never shipped: it only helps a debugger
const HIDDEN_PREFIX: char = '.'; // editor and tool litter such as .DS_Store and .vite/

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

/// Walk a build directory and check it, so a bad build fails here, before the
/// wasm build, with the code `--check-frontend` would give.
pub fn collect(dir: &Path) -> Result<Collected> {
    if !dir.exists() {
        bail!("frontend directory {} does not exist", dir.display());
    }
    if !dir.is_dir() {
        bail!("frontend directory {} is not a directory", dir.display());
    }
    let mut found = Vec::new();
    walk(dir, FRONTEND_ROOT.trim_end_matches('/'), &mut found)?;
    found.sort_by(|a, b| a.1.cmp(&b.1));
    if found.iter().map(|f| f.2).sum::<u64>() > MAX_FRONTEND_BYTES {
        return Err(FrontendError::TooLarge.into());
    }

    let mut files = Vec::with_capacity(found.len());
    let mut contents = Vec::with_capacity(found.len());
    for (source, rel, _) in found {
        let bytes = read_regular(&source, &rel)?;
        files.push(manifest::artifact_from_bytes(rel, &bytes));
        contents.push(bytes);
    }

    let frontend = BundleFrontend {
        entry: ENTRY.to_owned(),
        files,
    };
    frontend.validate()?;
    for (file, bytes) in frontend.files.iter().zip(&contents) {
        check_file(file, bytes)?;
    }
    Ok(Collected { frontend, contents })
}

/// Read one file, judging the handle it was opened as: a path swapped for a
/// pipe or link after the walk is refused, and a file that grew is cut at its length plus one.
fn read_regular(source: &Path, rel: &str) -> Result<Vec<u8>> {
    let read_err = || format!("failed to read {}", source.display());
    let file = File::open(source).wrap_err_with(read_err)?;
    let meta = file.metadata().wrap_err_with(read_err)?;
    if !meta.is_file() {
        return Err(FrontendError::UnsafePath(rel.to_owned()).into());
    }
    let len = meta.len();
    let mut bytes = Vec::new();
    let _ = file
        .take(len.saturating_add(1))
        .read_to_end(&mut bytes)
        .wrap_err_with(read_err)?;
    Ok(bytes)
}

/// The node's byte checks on one file, plus the build-time HTML lint.
fn check_file(file: &BundleArtifact, bytes: &[u8]) -> Result<()> {
    verify_frontend_file(file, bytes)?;
    if mime_type(&file.path) == mime_type(ENTRY) {
        check_frontend_html(&file.path, bytes)?;
    }
    Ok(())
}

/// Every regular file under `dir` as (source, archive path, size), dot-entries aside.
/// A symlink could point outside the build and a pipe or device could block a read, so both are refused.
fn walk(dir: &Path, rel: &str, out: &mut Vec<(PathBuf, String, u64)>) -> Result<()> {
    for entry in fs::read_dir(dir).wrap_err_with(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(HIDDEN_PREFIX) {
            continue;
        }
        let rel = format!("{rel}/{}", name.to_string_lossy());
        let meta = entry.metadata()?;
        if name.to_str().is_none() || !(meta.is_dir() || meta.is_file()) {
            return Err(FrontendError::UnsafePath(rel).into());
        }
        if meta.is_dir() {
            walk(&entry.path(), &rel, out)?;
        } else if !rel.ends_with(SOURCE_MAP_SUFFIX) {
            out.push((entry.path(), rel, meta.len()));
        }
    }
    Ok(())
}

/// An entry's archive path with `/` between components on every platform, so it
/// compares with the manifest's names even where `tar` yields backslashes.
fn archive_name(path: &Path) -> String {
    let parts: Vec<_> = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    parts.join("/")
}

fn open_archive(mpk: &Utf8Path) -> Result<tar::Archive<GzDecoder<File>>> {
    let file = File::open(mpk).wrap_err_with(|| format!("failed to open {mpk}"))?;
    Ok(tar::Archive::new(GzDecoder::new(file)))
}

/// The signed manifest's validated frontend section, from a pass that reads only `manifest.json`.
fn signed_frontend(mpk: &Utf8Path) -> Result<BundleFrontend> {
    let mut archive = open_archive(mpk)?;
    let mut manifest = None;
    for entry in archive
        .entries()
        .wrap_err("failed to read the bundle archive")?
    {
        let entry = entry?;
        if archive_name(&entry.path()?) != MANIFEST {
            continue;
        }
        let mut bytes = Vec::new();
        entry.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            bail!("{MANIFEST} exceeds {MAX_MANIFEST_BYTES} bytes");
        }
        manifest = Some(bytes);
    }

    let manifest = manifest.ok_or_else(|| eyre!("{mpk} has no {MANIFEST}"))?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest).wrap_err("manifest.json is not valid JSON")?;
    verify_manifest_signature(&manifest).wrap_err("the manifest signature does not verify")?;
    let manifest: BundleManifest =
        serde_json::from_value(manifest).wrap_err("manifest.json is not a bundle manifest")?;
    let frontend = manifest
        .frontend
        .ok_or_else(|| eyre!("the manifest declares no frontend"))?;
    frontend.validate()?;
    Ok(frontend)
}

/// Open an `.mpk`, verify its signature, validate the frontend it declares, then
/// check each declared file read to its declared size. An undeclared `frontend/`
/// member is refused unread.
pub fn check_mpk(mpk: &Utf8Path) -> Result<FrontendSummary> {
    let frontend = signed_frontend(mpk)?;
    let declared: HashMap<&str, &BundleArtifact> = frontend
        .files
        .iter()
        .map(|f| (f.path.as_str(), f))
        .collect();

    let mut seen = HashSet::new();
    let mut archive = open_archive(mpk)?;
    for entry in archive
        .entries()
        .wrap_err("failed to read the bundle archive")?
    {
        let mut entry = entry?;
        let name = archive_name(&entry.path()?);
        if !name.starts_with(FRONTEND_ROOT) {
            continue;
        }
        let Some(file) = declared.get(name.as_str()) else {
            return Err(FrontendError::UnsafePath(name).into());
        };
        if !entry.header().entry_type().is_file() {
            return Err(FrontendError::UnsafePath(name).into());
        }
        if !seen.insert(name.clone()) {
            return Err(FrontendError::DuplicatePath(name).into());
        }
        // One byte past the declared size, so an oversized file fails its size check.
        let mut bytes = Vec::new();
        (&mut entry)
            .take(file.size.saturating_add(1))
            .read_to_end(&mut bytes)?;
        check_file(file, &bytes)?;
    }

    if let Some(file) = frontend.files.iter().find(|f| !seen.contains(&f.path)) {
        return Err(FrontendError::MissingFile(file.path.clone()).into());
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
            "use plain files with names of letters, digits and . _ ~ @ + -; symlinks, pipes and devices are not allowed"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_join_components_with_a_slash_and_drop_a_leading_dot() {
        let path = Path::new(".")
            .join("frontend")
            .join("assets")
            .join("app.js");

        assert_eq!(archive_name(&path), "frontend/assets/app.js");
    }

    #[test]
    fn a_directory_in_a_files_place_is_refused_as_unsafe() {
        let tmp = tempfile::tempdir().unwrap();

        let err = read_regular(tmp.path(), "frontend/app.js").unwrap_err();

        assert!(
            matches!(
                err.downcast_ref::<FrontendError>(),
                Some(FrontendError::UnsafePath(p)) if p == "frontend/app.js"
            ),
            "{err:?}"
        );
    }
}
