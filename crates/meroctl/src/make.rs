//! `meroctl app` without a node: start a project, build it, generate its
//! clients, compare its ABI, make a signing key (`signer`), pack and sign an `.mpk`,
//! inspect one, and publish it to a registry. Each calls the library the
//! tool it replaced was built on (`calimero-app-build`, `calimero-app-abi`,
//! `mero-sign`, `calimero-bundle`), so the formats are the node's own.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use calimero_app_build::abi_tools::{self, DiffOutcome};
use calimero_bundle::{
    check_frontend_html, extract_bundle_manifest, format_bundle_hash, is_outbound_url, mime_type,
    BundleArtifact, BundleFrontend, VerifiedBundle, FRONTEND_ROOT,
};
use clap::{Args, Subcommand, ValueEnum};
use eyre::{bail, eyre, WrapErr};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The default component file names, as `mero-app-build` wrote them.
pub const APP_FILE: &str = "app.component.wasm";

/// Where an app's crate builds: `CARGO_TARGET_DIR` when set (as cargo does),
/// else `<dir>/target`.
pub fn target_dir(dir: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| dir.join("target"), PathBuf::from)
}
const COMPUTE_FILE: &str = "compute.component.wasm";
const WASM_MAGIC: &[u8] = b"\0asm\x0d\0\x01\0"; // a component; a core module is an SDK 0.12 app
const DEFAULT_MIN_RUNTIME: &str = "0.1.0";

#[derive(Debug, Args)]
pub struct NewArgs {
    /// The directory to create (it must not exist, or be empty).
    pub dir: PathBuf,
    /// The app under `apps/` to start from.
    #[arg(long, default_value = "kv-store")]
    pub template: String,
    /// The branch or tag of the apps repository.
    #[arg(long = "ref", default_value = "main")]
    pub git_ref: String,
    /// The apps repository to clone.
    #[arg(long, default_value = crate::dev::APPS_REPO, hide = true)]
    pub repo: String,
    /// Skip `git init` in the new project.
    #[arg(long)]
    pub no_git: bool,
}

#[derive(Debug, Args)]
pub struct BuildArgs {
    /// The app's crate (the directory holding its Cargo.toml).
    #[arg(default_value = ".")]
    pub dir: PathBuf,
    /// The app component (default: `<dir>/app.component.wasm`).
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// The compute component, when the app has compute methods (default:
    /// `compute.component.wasm` beside `--out`).
    #[arg(long)]
    pub compute_out: Option<PathBuf>,
    /// Cargo's target directory (default: `CARGO_TARGET_DIR`, else `<dir>/target`).
    #[arg(long)]
    pub target_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum AbiCmd {
    /// An ABI as JSON, read from a component, an `.mpk`, a `manifest.json` or ABI JSON.
    Show {
        file: PathBuf,
        /// The service of a multi-service bundle.
        #[arg(long)]
        service: Option<String>,
    },
    /// What changed between two releases (each a component, `.mpk` or ABI JSON); exits 1
    /// when a change breaks existing callers. `-- --help` prints its flags.
    #[command(disable_help_flag = true)]
    Diff {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        args: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SignerCmd {
    /// A new Ed25519 signing key, written owner-only (`mero-sign generate-key`).
    New {
        /// The key file to write.
        output: PathBuf,
        /// Replace an existing key file (the old key becomes unrecoverable).
        #[arg(long)]
        force: bool,
    },
    /// The `did:key` signer id of a key file: a package's identity is package + signer.
    Id { key: PathBuf },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Upgrades {
    Owner,
    Publisher,
}

#[derive(Debug, Args)]
pub struct PackArgs {
    /// The app's crate: reads `[package.metadata.calimero]` (`package`,
    /// `min-runtime-version`, `frontend-dir`) and the crate's `version`, and
    /// builds the components unless `--wasm` / `--compute` name them.
    pub dir: Option<PathBuf>,
    /// The app component (omit, with no DIR, for a compute-only package).
    #[arg(long)]
    pub wasm: Option<PathBuf>,
    /// The compute component of the same build, packed as `compute.wasm`.
    #[arg(long)]
    pub compute: Option<PathBuf>,
    /// A pipeline YAML file to ship (repeatable).
    #[arg(long = "pipeline")]
    pub pipelines: Vec<PathBuf>,
    /// A static web build directory, shipped as `frontend/` and checked with the node's rules.
    #[arg(long)]
    pub frontend: Option<PathBuf>,
    /// The page under `--frontend` served at `/`.
    #[arg(long, default_value = "index.html")]
    pub frontend_entry: String,
    /// Leave out the frontend DIR's metadata names.
    #[arg(long, conflicts_with = "frontend")]
    pub no_frontend: bool,
    /// An `https://` URL prefix the app's runs may send requests under (repeatable).
    #[arg(long = "outbound")]
    pub outbound: Vec<String>,
    /// Who upgrades the package's instances (absent: the owner).
    #[arg(long, value_enum)]
    pub upgrades: Option<Upgrades>,
    /// The package id, e.g. `com.example.kv-store` (default: DIR's metadata).
    #[arg(long)]
    pub package: Option<String>,
    /// The app's display name, `metadata.name` (default: DIR's `name`). A
    /// registry needs it, with a description, icon and category, to list a new
    /// package.
    #[arg(long)]
    pub name: Option<String>,
    /// A one-line description (default: DIR's `description`).
    #[arg(long)]
    pub description: Option<String>,
    /// A PNG icon, shipped as a data URI (default: DIR's `icon`, relative to DIR).
    #[arg(long)]
    pub icon: Option<PathBuf>,
    /// The storefront category, from the registry's list (default: DIR's `category`).
    #[arg(long)]
    pub category: Option<String>,
    /// The release's version (default: DIR's crate version).
    #[arg(long)]
    pub app_version: Option<String>,
    /// The oldest node release that runs it (default: DIR's metadata, else 0.1.0).
    #[arg(long)]
    pub min_runtime_version: Option<String>,
    /// The `.mpk` to write (default: `<DIR>/dist/<package>-<version>.mpk`, else in `.`).
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Sign with this key file (`meroctl app signer new`); without it, the
    /// development key, which a public registry refuses.
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// The last release (`.mpk`, `.wasm` or ABI JSON): warn about changes that
    /// break callers, refuse a state change without a state version bump.
    #[arg(long)]
    pub previous: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct SignArgs {
    /// The manifest.json to sign in place.
    pub manifest: PathBuf,
    /// The key file; without it `--dev` must be given.
    #[arg(long, short, conflicts_with = "dev")]
    pub key: Option<PathBuf>,
    /// The well-known development key (a public registry refuses it).
    #[arg(long)]
    pub dev: bool,
    /// The last release: report ABI changes first, as `pack --previous` does.
    #[arg(long)]
    pub previous: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PublishArgs {
    /// The signed `.mpk`.
    pub mpk: PathBuf,
    /// The registry.
    #[arg(
        long,
        env = "CALIMERO_REGISTRY_URL",
        default_value = "https://apps.calimero.network"
    )]
    pub registry: String,
    /// The registry API key (the registry's Organizations page, CLI Access),
    /// or with `--org` the Cloud organization's `ck_live_` key.
    #[arg(long, env = "CALIMERO_REGISTRY_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,
    /// Publish as a Calimero Cloud organization: the org key is traded at the
    /// Cloud for a short publish token first.
    #[arg(long)]
    pub org: Option<String>,
    /// The Cloud, for `--org`.
    #[arg(
        long,
        env = "CALIMERO_CLOUD_URL",
        default_value = "https://cloud.calimero.network"
    )]
    pub cloud: String,
    /// List the package publicly (a new package is private by default).
    #[arg(long)]
    pub public: bool,
}

/// `app new`, as `dev new` always did.
pub fn new(a: &NewArgs) -> eyre::Result<Value> {
    crate::dev::new(&a.dir, &a.template, &a.git_ref, &a.repo, !a.no_git)
}

/// `app build`: the app component (and the compute component when there is one).
pub fn build(a: &BuildArgs) -> eyre::Result<Value> {
    let out = a.out.clone().unwrap_or_else(|| a.dir.join(APP_FILE));
    let target = a.target_dir.clone().unwrap_or_else(|| target_dir(&a.dir));
    let built = calimero_app_build::build_components(&a.dir, &target)?;
    write_file(&out, &built.app.component)?;
    let mut answer = json!({ "app": out });
    match (&built.compute, &a.compute_out) {
        (Some(compute), path) => {
            let path = path
                .clone()
                .unwrap_or_else(|| out.with_file_name(COMPUTE_FILE));
            write_file(&path, &compute.component)?;
            answer["compute"] = json!(path);
        }
        (None, Some(_)) => bail!("{} has no compute methods", a.dir.display()),
        (None, None) => {}
    }
    Ok(answer)
}

/// `app codegen`: `abi_tools::codegen`, which prints what it wrote.
pub fn codegen(args: &[String]) -> eyre::Result<Value> {
    abi_tools::codegen(args.iter().cloned())?;
    Ok(Value::Null)
}

pub fn abi(cmd: &AbiCmd) -> eyre::Result<Value> {
    match cmd {
        AbiCmd::Show { file, service } => {
            Ok(calimero_app_abi::load::load_one(file, service.as_deref())?.json)
        }
        AbiCmd::Diff { args } => match abi_tools::abi_diff(args.iter().cloned())? {
            DiffOutcome::Compatible => Ok(Value::Null),
            DiffOutcome::Breaking => {
                std::io::stdout().flush()?;
                std::process::exit(1)
            }
        },
    }
}

pub fn signer(cmd: &SignerCmd) -> eyre::Result<Value> {
    match cmd {
        SignerCmd::New { output, force } => {
            mero_sign::generate_key(output, *force)?;
            Ok(Value::Null)
        }
        SignerCmd::Id { key } => {
            mero_sign::derive_signer_id(key)?;
            Ok(Value::Null)
        }
    }
}

fn signing_key(key: Option<&Path>) -> eyre::Result<ed25519_dalek::SigningKey> {
    match key {
        Some(k) => mero_sign::load_signing_key(k),
        None => Ok(mero_sign::dev_signing_key()),
    }
}

fn report_abi(manifest: &Path, previous: &Path) -> eyre::Result<()> {
    let check = mero_sign::check_abi(manifest, previous)?;
    eprintln!("ABI changes since {}:", previous.display());
    eprint!("{}", check.report);
    if check.breaking {
        eprintln!(
            "\u{26a0}  This release breaks existing callers of the app (see `breaking` above)."
        );
    }
    Ok(())
}

pub fn sign(a: &SignArgs) -> eyre::Result<Value> {
    if a.key.is_none() && !a.dev {
        bail!("give --key <file>, or --dev for the development key");
    }
    if let Some(previous) = &a.previous {
        report_abi(&a.manifest, previous)?;
    }
    mero_sign::sign_manifest(&a.manifest, &signing_key(a.key.as_deref())?)?;
    Ok(json!({ "signed": a.manifest }))
}

/// `app inspect`: open a bundle as a node does (signature, every artifact's
/// hash) and print its manifest and signer.
pub fn inspect(mpk: &Path) -> eyre::Result<Value> {
    let data: Arc<[u8]> = std::fs::read(mpk)
        .wrap_err_with(|| format!("reading {}", mpk.display()))?
        .into();
    let release = semver::Version::parse(env!("CALIMERO_RELEASE_VERSION"))
        .unwrap_or_else(|_| semver::Version::new(u64::MAX, 0, 0));
    let bundle = VerifiedBundle::open(data, &release)?;
    let _ = bundle.contents()?;
    Ok(json!({
        "verified": true,
        "signerId": bundle.signer_id(),
        "bundleHash": format_bundle_hash(&bundle.manifest_hash()),
        "manifest": bundle.manifest(),
    }))
}

/// A file as it goes into the archive: its path there and its bytes.
type ArchiveFile = (String, Vec<u8>);

/// What a crate's Cargo.toml says about its package: `[package] version` and
/// `[package.metadata.calimero]`.
#[derive(Default)]
struct CrateMeta {
    version: Option<String>,
    package: Option<String>,
    min_runtime: Option<String>,
    frontend_dir: Option<PathBuf>,
    /// `[package.metadata.calimero]` as written, for the display metadata,
    /// links and the rest `pack` copies into the manifest (`manifest_extras`).
    table: toml::Table,
    /// Where `calimero-sdk` comes from (`[dependencies]`, a git source and its
    /// tag), written into the manifest's `buildInfo` as `mero-component` did.
    sdk: Option<(String, Option<String>)>,
    dir: PathBuf,
}

fn crate_meta(dir: &Path) -> eyre::Result<CrateMeta> {
    let path = dir.join("Cargo.toml");
    let text =
        std::fs::read_to_string(&path).wrap_err_with(|| format!("reading {}", path.display()))?;
    let doc: toml::Table = text
        .parse()
        .wrap_err_with(|| format!("parsing {}", path.display()))?;
    let package = doc.get("package").and_then(toml::Value::as_table);
    let meta = package
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("calimero"))
        .and_then(toml::Value::as_table);
    let text_of = |t: Option<&toml::Table>, k: &str| {
        t.and_then(|t| t.get(k))
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    };
    // A component app takes `calimero-sdk` from core's git; one inherited from
    // a workspace (`workspace = true`) names no source here.
    let sdk = doc
        .get("dependencies")
        .and_then(|d| d.get("calimero-sdk"))
        .and_then(toml::Value::as_table)
        .and_then(|d| {
            let git = d.get("git")?.as_str()?.to_owned();
            Some((git, text_of(Some(d), "tag")))
        });
    Ok(CrateMeta {
        sdk,
        version: text_of(package, "version"),
        package: text_of(meta, "package"),
        min_runtime: text_of(meta, "min-runtime-version"),
        frontend_dir: text_of(meta, "frontend-dir").map(|d| dir.join(d)),
        table: meta.cloned().unwrap_or_default(),
        dir: dir.to_owned(),
    })
}

/// A PNG file as the data URI `metadata.icon` carries.
fn icon_uri(path: &Path) -> eyre::Result<String> {
    use base64::Engine as _;
    let bytes = std::fs::read(path).wrap_err_with(|| format!("reading icon {}", path.display()))?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        bail!("icon {} is not a PNG", path.display());
    }
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// What the manifest carries besides the artifacts, from the crate's
/// `[package.metadata.calimero]` and the flags, the keys `mero-component bundle`
/// (and `cargo mero` before it) read: `metadata` (`name`, `description`,
/// `author`, `icon`, `tags`, `license`, `category`, `guide`), `handlers.slug`
/// (default: the package), `links` (`frontend`, `github`, `docs`),
/// `frontendAliases`, `outbound` and `upgrades`, and `buildInfo` (the
/// `calimero-sdk` git source and tag). A flag wins over the table.
fn manifest_extras(
    a: &PackArgs,
    meta: &CrateMeta,
    package: &str,
    manifest: &mut Map<String, Value>,
) -> eyre::Result<()> {
    let t = &meta.table;
    let text = |k: &str| t.get(k).and_then(toml::Value::as_str).map(str::to_owned);
    let list = |k: &str| -> Vec<String> {
        t.get(k)
            .and_then(toml::Value::as_array)
            .map(|v| {
                v.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let icon = match (&a.icon, text("icon")) {
        (Some(p), _) => Some(icon_uri(p)?),
        (None, Some(rel)) if rel == "default" => {
            eprintln!(
                "note: icon = \"default\" names cargo-mero's built-in mark; packed without an icon"
            );
            None
        }
        (None, Some(rel)) => Some(icon_uri(&meta.dir.join(rel))?),
        (None, None) => None,
    };
    let guide = text("guide")
        .map(|rel| {
            let path = meta.dir.join(rel);
            std::fs::read_to_string(&path)
                .wrap_err_with(|| format!("reading guide {}", path.display()))
        })
        .transpose()?;
    if let Some(name) = a.name.clone().or_else(|| text("name")) {
        let mut m = Map::new();
        let _ = m.insert("name".into(), json!(name));
        let mut put = |k: &str, v: Option<String>| {
            if let Some(v) = v {
                let _ = m.insert(k.into(), json!(v));
            }
        };
        put(
            "description",
            a.description.clone().or_else(|| text("description")),
        );
        put("author", text("author"));
        put("icon", icon);
        put("license", text("license"));
        put("category", a.category.clone().or_else(|| text("category")));
        put("guide", guide);
        let _ = m.insert("tags".into(), json!(list("tags")));
        let _ = manifest.insert("metadata".into(), Value::Object(m));
    } else if a.description.is_some() || a.icon.is_some() || a.category.is_some() {
        bail!("--description, --icon and --category describe the app under its --name: give --name too");
    }
    let _ = manifest.insert(
        "handlers".into(),
        json!({ "slug": text("slug").unwrap_or_else(|| package.to_owned()) }),
    );
    let mut links = Map::new();
    for k in ["frontend", "github", "docs"] {
        if let Some(v) = text(k) {
            if !calimero_bundle::is_web_url(&v) {
                bail!("{k} = {v:?} is not an http(s) URL");
            }
            let _ = links.insert(k.into(), json!(v));
        }
    }
    if !links.is_empty() {
        let _ = manifest.insert("links".into(), Value::Object(links));
    }
    if let Some((source, version)) = &meta.sdk {
        let mut b = Map::new();
        let _ = b.insert("sdkSource".into(), json!(source));
        if let Some(v) = version {
            let _ = b.insert("sdkVersion".into(), json!(v));
        }
        let _ = manifest.insert("buildInfo".into(), Value::Object(b));
    }
    let aliases = list("frontend-aliases");
    if !aliases.is_empty() {
        let _ = manifest.insert("frontendAliases".into(), json!(aliases));
    }
    Ok(())
}

fn artifact(path: &str, data: &[u8]) -> BundleArtifact {
    BundleArtifact {
        path: path.to_owned(),
        hash: hex::encode(Sha256::digest(data)),
        size: data.len() as u64,
    }
}

/// Every file under `root` as `frontend/<path>`, sorted, refused as the node
/// would refuse the manifest (`BundleFrontend::validate`).
fn frontend_files(root: &Path, entry: &str) -> eyre::Result<(BundleFrontend, Vec<ArchiveFile>)> {
    if !root.is_dir() {
        bail!("--frontend {}: not a directory", root.display());
    }
    let mut stack = vec![root.to_owned()];
    let mut found = Vec::new();
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                found.push(p);
            }
        }
    }
    found.sort();
    let mut files = Vec::new();
    for f in found {
        let rel = f.strip_prefix(root)?;
        let rel = rel
            .components()
            .map(|c| c.as_os_str().to_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| eyre!("--frontend: {} is not UTF-8", f.display()))?
            .join("/");
        files.push((format!("{FRONTEND_ROOT}{rel}"), std::fs::read(&f)?));
    }
    let frontend = BundleFrontend {
        entry: format!("{FRONTEND_ROOT}{entry}"),
        files: files.iter().map(|(p, d)| artifact(p, d)).collect(),
    };
    frontend.validate().map_err(|e| eyre!("--frontend: {e}"))?;
    // The node only checks paths, sizes and types; the HTML rules (no remote or
    // escaping URL, no inline code) are what the served page's CSP enforces.
    for (path, data) in &files {
        if mime_type(path) == Some("text/html; charset=utf-8") || path.ends_with(".html") {
            check_frontend_html(path, data).map_err(|e| eyre!("--frontend: {e}"))?;
        }
    }
    Ok((frontend, files))
}

fn component(path: &Path) -> eyre::Result<Vec<u8>> {
    let data = std::fs::read(path).wrap_err_with(|| format!("reading {}", path.display()))?;
    if !data.starts_with(WASM_MAGIC) {
        bail!(
            "{}: not a WASM component (an SDK 0.12 core module is refused by the node)",
            path.display()
        );
    }
    Ok(data)
}

/// `app pack`: the manifest the node reads, signed, and every file it names, as a `.mpk`.
pub fn pack(a: &PackArgs) -> eyre::Result<Value> {
    let meta = a
        .dir
        .as_deref()
        .map(crate_meta)
        .transpose()?
        .unwrap_or_default();
    let package = a
        .package
        .clone()
        .or_else(|| meta.package.clone())
        .ok_or_else(|| {
            eyre!(
                "give --package, or a DIR whose Cargo.toml has [package.metadata.calimero] package"
            )
        })?;
    let version = a
        .app_version
        .clone()
        .or_else(|| meta.version.clone())
        .ok_or_else(|| eyre!("give --app-version, or a DIR whose crate has a version"))?;
    let min_runtime = a
        .min_runtime_version
        .clone()
        .or_else(|| meta.min_runtime.clone())
        .unwrap_or_else(|| DEFAULT_MIN_RUNTIME.to_owned());

    // The components: named, or built from DIR.
    let (mut wasm, mut compute) = (
        a.wasm.as_deref().map(component).transpose()?,
        a.compute.as_deref().map(component).transpose()?,
    );
    if let (Some(dir), None, None) = (&a.dir, &wasm, &compute) {
        eprintln!("building {}", dir.display());
        let built = calimero_app_build::build_components(dir, &target_dir(dir))?;
        wasm = Some(built.app.component);
        compute = built.compute.map(|c| c.component);
    }
    if wasm.is_none() && compute.is_none() {
        bail!("give --wasm, --compute or both, or the app's crate DIR");
    }
    let mut parts: Vec<(&str, &str, Vec<u8>)> = Vec::new();
    if let Some(w) = wasm {
        parts.push(("wasm", "app.wasm", w));
    }
    if let Some(c) = compute {
        parts.push(("compute", "compute.wasm", c));
    }

    let mut pipelines = Vec::new();
    for f in &a.pipelines {
        let name = f
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| eyre!("--pipeline {}: no file name", f.display()))?;
        pipelines.push((format!("pipelines/{name}"), std::fs::read(f)?));
    }
    let mut names: Vec<_> = pipelines.iter().map(|(n, _)| n).collect();
    names.sort();
    names.dedup();
    if names.len() != pipelines.len() {
        bail!("two --pipeline files have the same name");
    }

    let frontend_dir = match (&a.frontend, a.no_frontend, meta.frontend_dir.clone()) {
        (Some(d), _, _) => Some(d.clone()),
        (None, false, Some(d)) if d.is_dir() => Some(d),
        (None, false, Some(d)) => {
            eprintln!(
                "note: no frontend packed: {} (frontend-dir) does not exist; build the frontend first, or pass --no-frontend",
                d.display()
            );
            None
        }
        _ => None,
    };
    let frontend = frontend_dir
        .map(|d| frontend_files(&d, &a.frontend_entry))
        .transpose()?;

    for url in &a.outbound {
        if !is_outbound_url(url) {
            bail!("--outbound {url}: an https:// prefix with a host, no credentials, query or fragment");
        }
    }

    let mut manifest = Map::new();
    let _ = manifest.insert("version".into(), json!("1.0"));
    let _ = manifest.insert("package".into(), json!(package));
    let _ = manifest.insert("appVersion".into(), json!(version));
    let _ = manifest.insert("minRuntimeVersion".into(), json!(min_runtime));
    for (key, name, data) in &parts {
        let _ = manifest.insert((*key).into(), serde_json::to_value(artifact(name, data))?);
    }
    if !pipelines.is_empty() {
        let list: Vec<_> = pipelines.iter().map(|(n, d)| artifact(n, d)).collect();
        let _ = manifest.insert("pipelines".into(), serde_json::to_value(list)?);
    }
    if let Some((f, _)) = &frontend {
        let _ = manifest.insert("frontend".into(), serde_json::to_value(f)?);
    }
    manifest_extras(a, &meta, &package, &mut manifest)?;
    let mut outbound: Vec<String> = meta
        .table
        .get("outbound")
        .and_then(toml::Value::as_array)
        .map(|v| {
            v.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    for url in &a.outbound {
        if !outbound.contains(url) {
            outbound.push(url.clone());
        }
    }
    for url in &outbound {
        if !is_outbound_url(url) {
            bail!(
                "outbound {url}: an https:// prefix with a host, no credentials, query or fragment"
            );
        }
    }
    if !outbound.is_empty() {
        let _ = manifest.insert("outbound".into(), json!(outbound));
    }
    let upgrades = match a.upgrades {
        Some(Upgrades::Owner) => Some("owner".to_owned()),
        Some(Upgrades::Publisher) => Some("publisher".to_owned()),
        None => meta
            .table
            .get("upgrades")
            .and_then(toml::Value::as_str)
            .map(str::to_owned),
    };
    if let Some(u) = upgrades {
        if u != "owner" && u != "publisher" {
            bail!("upgrades = {u:?}: owner or publisher");
        }
        let _ = manifest.insert("upgrades".into(), json!(u));
    }

    let key = signing_key(a.key.as_deref())?;
    if a.key.is_none() {
        eprintln!(
            "signing with the development key (a public registry refuses it; --key for yours)"
        );
    }
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("manifest.json");
    std::fs::write(&path, serde_json::to_vec(&Value::Object(manifest))?)?;
    // Beside the manifest, where `--previous` reads the new ABI from.
    for (_, name, data) in &parts {
        std::fs::write(tmp.path().join(name), data)?;
    }
    if let Some(previous) = &a.previous {
        report_abi(&path, &std::path::absolute(previous)?)?;
    }
    mero_sign::sign_manifest(&path, &key)?;
    let signed = std::fs::read(&path)?;

    let out = a.out.clone().unwrap_or_else(|| {
        let file = format!("{package}-{version}.mpk");
        match &a.dir {
            Some(d) => d.join("dist").join(file),
            None => PathBuf::from(file),
        }
    });
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let gz =
        flate2::write::GzEncoder::new(std::fs::File::create(&out)?, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    let mut entries: Vec<(&str, &[u8])> = vec![("manifest.json", &signed)];
    entries.extend(parts.iter().map(|(_, n, d)| (*n, d.as_slice())));
    entries.extend(pipelines.iter().map(|(n, d)| (n.as_str(), d.as_slice())));
    if let Some((_, files)) = &frontend {
        entries.extend(files.iter().map(|(n, d)| (n.as_str(), d.as_slice())));
    }
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, name, data)?;
    }
    tar.into_inner()?.finish()?;
    Ok(json!({ "wrote": out, "package": package, "appVersion": version }))
}

/// `app publish`: the registry's `POST /api/v2/bundles/push`, the signed
/// manifest's own fields (all of them: the signature covers every key) with
/// the whole `.mpk` as hex in `_binary`.
pub async fn publish(a: &PublishArgs) -> eyre::Result<Value> {
    let data = std::fs::read(&a.mpk).wrap_err_with(|| format!("reading {}", a.mpk.display()))?;
    let (manifest, _) = extract_bundle_manifest(&data, &semver::Version::new(u64::MAX, 0, 0))?;
    let Value::Object(mut body) = manifest else {
        bail!("{}: the manifest is not an object", a.mpk.display());
    };
    let _ = body.entry("version").or_insert_with(|| json!("1.0"));
    let _ = body.entry("migrations").or_insert_with(|| json!([]));
    let _ = body.insert("_binary".into(), json!(hex::encode(&data)));
    if a.public {
        let _ = body.insert("_visibility".into(), json!("public"));
    }
    let key = a
        .api_key
        .clone()
        .ok_or_else(|| eyre!("give --api-key or CALIMERO_REGISTRY_API_KEY"))?;
    let registry = a.registry.trim_end_matches('/');
    if !registry.starts_with("https://") && !is_loopback(registry) {
        bail!("--registry must be https:// (an API key is never sent in the clear)");
    }
    let http = reqwest::Client::new();
    let token = match &a.org {
        Some(org) => registry_token(&http, &a.cloud, &key, org).await?,
        None => key,
    };
    let res = http
        .post(format!("{registry}/api/v2/bundles/push"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await?;
    let status = res.status();
    let answer: Value = res.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let code = answer["error"].as_str().unwrap_or("error");
        let message = answer["message"].as_str().unwrap_or_default();
        bail!("the registry refused the bundle: {status} {code}: {message}");
    }
    Ok(answer)
}

pub fn is_loopback(url: &str) -> bool {
    ["http://127.0.0.1", "http://localhost", "http://[::1]"]
        .iter()
        .any(|p| url.starts_with(p))
}

fn write_file(out: &Path, bytes: &[u8]) -> eyre::Result<()> {
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out, bytes).wrap_err_with(|| format!("writing {}", out.display()))
}

/// Trade a Cloud organization's API key (`ck_live_…`, scope `registry:publish`)
/// for the short-lived token the registry takes from that organization:
/// `POST /api/v2/registry/token`.
async fn registry_token(
    http: &reqwest::Client,
    cloud: &str,
    org_key: &str,
    org: &str,
) -> eyre::Result<String> {
    let cloud = cloud.trim_end_matches('/');
    let res = http
        .post(format!("{cloud}/api/v2/registry/token"))
        .bearer_auth(org_key)
        .json(&json!({ "org": org, "scope": "publish" }))
        .send()
        .await?;
    let status = res.status();
    let answer: Value = res.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!(
            "the Cloud refused the organization key: {status} {}",
            answer["detail"]
                .as_str()
                .or(answer["message"].as_str())
                .unwrap_or_default()
        );
    }
    answer["token"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| eyre!("the Cloud answered no token"))
}
