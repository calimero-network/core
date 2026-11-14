//! An app's typed client and OpenAPI document from its ABI, in process: what
//! `meroctl app codegen` runs.
//!
//! ```no_run
//! use calimero_app_build::codegen::{Codegen, Lang};
//!
//! let mut job = Codegen::new("logic/res/abi.json", Lang::TypeScript, "app/src/generated");
//! job.name = Some("kv-store".to_owned());
//! for path in job.run()? {
//!     println!("wrote {}", path.display());
//! }
//! # Ok::<(), eyre::Report>(())
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use calimero_app_abi::codegen::{
    kotlin, kotlin_package, python, swift, typescript, KotlinOptions, Target,
};
use calimero_app_abi::load::{load_one, load_path, Loaded};
use calimero_app_abi::openapi::{document, Info};
use eyre::{bail, eyre, Result as EyreResult};

/// File stems that name no app: what a build writes (`abi.json`, `app.component.wasm`) rather
/// than what the app is called. A client named after one would be `AbiClient` in every app.
const GENERIC_STEMS: &[&str] = &["abi", "app", "component", "compute", "manifest"];

/// The client's language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Lang {
    /// `client.ts` over mero-js.
    TypeScript,
    /// `client.py` over calimero-client-py.
    Python,
    /// `<Name>Client.swift` over swift-sdk.
    Swift,
    /// `<Name>Client.kt` over kotlin-sdk.
    Kotlin,
    /// No client: the OpenAPI document alone.
    OpenApi,
}

impl FromStr for Lang {
    type Err = eyre::Report;

    fn from_str(s: &str) -> EyreResult<Self> {
        Ok(match s {
            "ts" | "typescript" => Self::TypeScript,
            "python" | "py" => Self::Python,
            "swift" => Self::Swift,
            "kotlin" | "kt" => Self::Kotlin,
            "openapi" => Self::OpenApi,
            other => bail!("unknown --lang {other:?}"),
        })
    }
}

/// One codegen run. [`Codegen::new`] sets what is required; the rest are the CLI's optional
/// flags, left as they come.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Codegen {
    /// `abi.json`, a component, an `.mpk` or a bundle's `manifest.json`.
    pub abi: PathBuf,
    pub lang: Lang,
    /// The directory written; a multi-service bundle gets a subdirectory per service.
    pub out: PathBuf,
    /// The app's name (`kv-store` makes `KvStoreClient`); see [`Codegen::run`] when `None`.
    pub name: Option<String>,
    /// One service of a multi-service bundle; `None` writes every service.
    pub service: Option<String>,
    /// The Kotlin package.
    pub package: Option<String>,
    /// Whether the Kotlin client is compiled with the kotlinx.serialization plugin
    /// ([`KotlinOptions::serialization_plugin`]).
    pub kotlin_serialization_plugin: bool,
    /// Whether to write `openapi.json` beside the client.
    pub openapi: bool,
}

impl Codegen {
    pub fn new(abi: impl Into<PathBuf>, lang: Lang, out: impl Into<PathBuf>) -> Self {
        Self {
            abi: abi.into(),
            lang,
            out: out.into(),
            name: None,
            service: None,
            package: None,
            kotlin_serialization_plugin: false,
            openapi: true,
        }
    }

    /// Writes the client (and `openapi.json` unless `openapi` is off) and returns the paths
    /// written.
    ///
    /// Without `name`, the client is named after the service, else the bundle's package
    /// (`com.example.kv-store` → `kv-store`), else the file (`kv-store.abi.json`). A file
    /// named only for what it holds (`abi.json`, `app.component.wasm`) is named after the
    /// crate whose `Cargo.toml` is nearest above it; with none, the run asks for `name`.
    pub fn run(&self) -> EyreResult<Vec<PathBuf>> {
        if !self.openapi && self.lang == Lang::OpenApi {
            bail!("--lang openapi with --no-openapi writes nothing");
        }
        let all = match &self.service {
            Some(s) => vec![load_one(&self.abi, Some(s))?],
            None => load_path(&self.abi)?,
        };
        let several = all.len() > 1;
        let mut jobs = Vec::new();
        // Every name first, so a run that cannot name one service writes none.
        for loaded in &all {
            let dir = match (&loaded.service, several) {
                (Some(s), true) => self.out.join(s),
                _ => self.out.clone(),
            };
            let name = match self
                .name
                .clone()
                .or_else(|| loaded.service.clone())
                .or_else(|| loaded.package.as_deref().map(last_segment))
            {
                Some(name) => name,
                None => name_of_file(&self.abi)?,
            };
            // Each service's Kotlin client gets a package of its own, as each gets a directory.
            let package = match (&self.package, &loaded.service, several) {
                (Some(p), Some(s), true) => Some(format!("{p}.{}", kotlin_package_segment(s))),
                (Some(p), _, _) => Some(p.clone()),
                (None, _, _) => None,
            };
            jobs.push((loaded, dir, name, package));
        }
        let mut written = Vec::new();
        for (loaded, dir, name, package) in jobs {
            self.write_outputs(loaded, &dir, &name, package.as_deref(), &mut written)?;
        }
        Ok(written)
    }

    fn write_outputs(
        &self,
        loaded: &Loaded,
        dir: &Path,
        name: &str,
        package: Option<&str>,
        written: &mut Vec<PathBuf>,
    ) -> EyreResult<()> {
        fs::create_dir_all(dir)?;
        let mut write = |path: PathBuf, text: &str| -> EyreResult<()> {
            fs::write(&path, text).map_err(|e| eyre!("{}: {e}", path.display()))?;
            written.push(path);
            Ok(())
        };
        if self.openapi {
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
                dir.join("openapi.json"),
                &format!("{}\n", serde_json::to_string_pretty(&doc)?),
            )?;
        }
        let target = Target { name };
        match self.lang {
            Lang::OpenApi => {}
            Lang::TypeScript => write(dir.join("client.ts"), &typescript(&loaded.abi, &target))?,
            Lang::Python => write(dir.join("client.py"), &python(&loaded.abi, &target))?,
            Lang::Swift => write(
                dir.join(format!("{}.swift", target.class())),
                &swift(&loaded.abi, &target),
            )?,
            Lang::Kotlin => write(
                dir.join(format!("{}.kt", target.class())),
                &kotlin(
                    &loaded.abi,
                    &target,
                    &KotlinOptions {
                        package,
                        serialization_plugin: self.kotlin_serialization_plugin,
                    },
                ),
            )?,
        }
        Ok(())
    }
}

/// The app's name from the ABI's file: `kv-store.abi.json` → `kv-store`; `abi.json` → the
/// package name of the nearest `Cargo.toml` above it.
fn name_of_file(path: &Path) -> EyreResult<String> {
    let stem = stem(path);
    if !stem.is_empty() && !GENERIC_STEMS.contains(&stem.as_str()) {
        return Ok(stem);
    }
    let absolute = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let manifest = absolute
        .ancestors()
        .skip(1)
        .map(|dir| dir.join("Cargo.toml"))
        .find(|m| m.is_file());
    if let Some(manifest) = &manifest {
        if let Some(name) = package_name(manifest) {
            return Ok(name);
        }
    }
    let nearest = manifest.map_or_else(
        || "no Cargo.toml above it".to_owned(),
        |m| format!("{} names no [package]", m.display()),
    );
    bail!(
        "cannot name the client after {}: the ABI carries no app name, {stem:?} names none \
         either, and {nearest}; pass --name <app> (e.g. --name kv-store for KvStoreClient)",
        path.display()
    )
}

fn package_name(manifest: &Path) -> Option<String> {
    let text = fs::read_to_string(manifest).ok()?;
    let doc: toml::Table = text.parse().ok()?;
    doc.get("package")?.get("name")?.as_str().map(str::to_owned)
}

/// `room-directory` → `roomdirectory`: the last segment of a service's Kotlin package.
fn kotlin_package_segment(service: &str) -> String {
    kotlin_package(service)
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_owned()
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
