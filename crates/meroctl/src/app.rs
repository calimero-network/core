//! `meroctl app` makes and ships an app's signed package (v1's application):
//! making one (`new`, `build`, `codegen`, `abi`) and shipping it (`signer`,
//! `pack`, `sign`, `inspect`, `publish`) need no node (`make.rs`); `release`
//! packs and installs in one step. `meroctl package` ([`PackageCmd`]) handles
//! the packages installed on the cluster (verified on every node); each
//! running copy of one is an instance (`meroctl instance create --package`).
//! `meroctl app install` and the rest of `package`'s commands still parse,
//! hidden from `app`'s help. `check` judges a release against the one it
//! replaces (`app/check.rs`): offline against a `.mpk`, with a node against an
//! installed version.

use std::path::{Path, PathBuf};
use std::time::Duration;

use calimero_client::types::AppUpgradeRequest;
use clap::Subcommand;
use serde_json::{json, Value};

use crate::env::{load_dev_passkey, read_file, save_passkey, to_json, DevPasskey, Env, Home};
use crate::registry::{Registry, DEFAULT_REGISTRY};

pub mod check;

#[derive(Debug, Subcommand)]
pub enum AppCmd {
    /// Start a project in DIR from an app of the apps repository (default:
    /// the KV Store reference app, contract and frontend). Needs git; no node.
    New(crate::make::NewArgs),
    /// Build the app's crate into a `calimero:app` component (and its compute
    /// component, when it has compute methods). No node.
    Build(crate::make::BuildArgs),
    /// Typed clients and the OpenAPI document from an ABI: `--abi FILE --lang
    /// ts|python|swift|kotlin|openapi [-o DIR] [--name N]`; `-- --help` for every flag. No node.
    #[command(disable_help_flag = true)]
    Codegen {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        args: Vec<String>,
    },
    /// An app's ABI: print it, or compare two releases. No node.
    #[command(subcommand)]
    Abi(crate::make::AbiCmd),
    /// The apps the crate `uses`: pin each to a release and vendor its ABI
    /// (`update`), or check the lock and the vendored ABIs (`check`); the
    /// crate's `build.rs` generates their typed clients from them. No node.
    #[command(subcommand)]
    Uses(crate::make::uses::UsesCmd),
    /// The signing key a release is signed with: a package's identity is its
    /// package id and its signer. No node.
    #[command(subcommand)]
    Signer(crate::make::SignerCmd),
    /// Write a signed `.mpk`: from the app's crate DIR (its
    /// `[package.metadata.calimero]`), or from `--wasm`/`--compute` and flags. No node.
    Pack(Box<crate::make::PackArgs>),
    /// Sign a manifest.json in place. No node.
    Sign(crate::make::SignArgs),
    /// Open an `.mpk` as a node does (signature and every hash) and print its manifest. No node.
    Inspect { mpk: PathBuf },
    /// Publish a signed `.mpk` to the app registry, or to a Calimero Cloud
    /// organization (`--org`). No node.
    Publish(crate::make::PublishArgs),
    /// What people running the app will see when this release replaces an
    /// earlier one: green or red and why, where its data goes, new
    /// methods, state version, signer. `--fail-on red|egress` makes it a
    /// release gate (exit 1; 2 when it cannot judge). No node with `--against
    /// FILE.mpk`; `--against VERSION` reads the node's installed release.
    Check(Box<check::CheckArgs>),
    /// Ship a release in one step: `pack` (build from DIR, default `.`, and
    /// sign), then install it on the cluster, ready for `meroctl deployment
    /// offer`: uploaded (development clusters), or with `--publish` pushed
    /// to the registry (`publish`) and installed from the cluster's registry. Refuses a version
    /// that is not newer than every release of the package the cluster has
    /// installed, before building anything.
    Release(Box<crate::make::ReleaseArgs>),
    /// The packages installed on the cluster (`meroctl package`): hidden
    /// here, where they parse as before.
    #[command(flatten)]
    Package(PackageCmd),
}

/// `meroctl package`: the packages installed on the cluster.
#[derive(Debug, Subcommand)]
pub enum PackageCmd {
    /// Install a package: `PACKAGE@VERSION` (or `PACKAGE --version V`)
    /// from the node's registry, or a local file with `--path` (a signed
    /// `.mpk`; on a development cluster also a raw `.wasm`, which uploads a
    /// bundle). `PACKAGE` alone installs the latest version `--registry`
    /// lists (`meroctl registry versions`), which should be the registry the
    /// node installs from.
    Install {
        #[arg(value_name = "PACKAGE[@VERSION]", required_unless_present = "path")]
        coords: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, conflicts_with = "coords")]
        path: Option<PathBuf>,
        /// The organization it is installed for (default: your only one). A private
        /// release, and every upload, is used only by its members.
        #[arg(long)]
        org: Option<String>,
        /// The registry a version-less `PACKAGE` is looked up in.
        #[arg(long, env = "CALIMERO_REGISTRY_URL", default_value = DEFAULT_REGISTRY)]
        registry: String,
        /// Its API key, for a private package (default: the one `meroctl
        /// registry login` saved).
        #[arg(long, env = "CALIMERO_REGISTRY_API_KEY", hide_env_values = true)]
        registry_api_key: Option<String>,
    },
    /// Installed packages and their versions.
    #[command(alias = "ls", alias = "list-packages")]
    List,
    /// One installed package.
    Get { package: String },
    /// A package's installed versions.
    #[command(alias = "list-versions")]
    Versions { package: String },
    /// A package's newest installed version.
    GetLatestVersion { package: String },
    /// The pipelines a version registers (default: the newest installed): name, hash,
    /// worst-case fuel, the modules it pins and the canonical pipeline.
    Pipelines {
        package: String,
        #[arg(long)]
        version: Option<String>,
    },
    /// Uninstall one version, or the whole package. Refused while an instance runs it.
    Uninstall {
        package: String,
        #[arg(long)]
        version: Option<String>,
    },
    /// Push a build to its instances now, then on every save. A signed `.mpk` is installed and the instances running its
    /// package are updated to its version; a raw `.wasm` (development
    /// clusters) updates the instances named with `--instance`.
    Watch(WatchArgs),
    /// The web origins (a developer's own site, a hosting preview) whose
    /// pages may sign people in to this package's frontend, each under its
    /// own passkey rp-id. Its manager's: the publisher organization's
    /// holders of the capability to manage apps, else whoever installed it first.
    #[command(subcommand)]
    Origins(OriginsCmd),
}

#[derive(Debug, Subcommand)]
pub enum OriginsCmd {
    /// The origins registered for a package, with their rp-ids and
    /// whether the cluster serves its frontend there (`served`).
    #[command(alias = "ls")]
    List { package: String },
    /// Register an origin (`https://host[:port]`, as a browser writes it).
    /// The manager approves with a passkey: `--dev-passkey` on a development
    /// cluster; without it the node answers `412 CONFIRM_REQUIRED` and the
    /// change is made in the console (Web addresses).
    Add {
        package: String,
        origin: String,
        /// The passkey rp-id ceremonies there use: the origin's host (the
        /// default) or a parent domain of it, never the cluster's.
        #[arg(long)]
        rp_id: Option<String>,
        /// Have the cluster serve the package's frontend on this origin
        /// too (a custom domain: `https://host`, no port). The approval names
        /// it, and every node of the cluster must run this release or later.
        #[arg(long)]
        served: bool,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Remove an origin; the sessions opened there end. Approved as `add`.
    #[command(alias = "rm")]
    Remove {
        package: String,
        origin: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

/// What `package watch` and `dev watch` push, where, and how often.
#[derive(Debug, clap::Args)]
pub struct WatchArgs {
    #[arg(long)]
    pub path: PathBuf,
    /// The instances to update (default for a `.mpk`: this account's
    /// instances of its package).
    #[arg(long = "instance", short = 'i', value_name = "INSTANCE")]
    pub contexts: Vec<String>,
    /// The owner's passkey file that approves each update.
    #[arg(long)]
    pub dev_passkey: PathBuf,
    /// Push once and exit.
    #[arg(long)]
    pub once: bool,
    /// How often to look at the file, in milliseconds.
    #[arg(long, default_value_t = 500)]
    pub interval_ms: u64,
    /// The organization a `.mpk` is installed for (default: your only one).
    #[arg(long)]
    pub org: Option<String>,
}

/// Install a local file: a `.mpk` as a package (for `org`), anything else as a raw bundle.
pub async fn install_file(env: &Env, path: &Path, org: Option<&str>) -> eyre::Result<Value> {
    let bytes = read_file(path)?;
    if path.extension().is_some_and(|e| e == "mpk") {
        to_json(&env.client.packages().upload(bytes, org).await?)
    } else {
        let hash = env.client.packages().bundle_put(bytes).await?.hash;
        Ok(json!({ "bundle": hash }))
    }
}

pub fn split_coords(coords: &str, version: Option<String>) -> eyre::Result<(String, String)> {
    match coords.split_once('@') {
        Some((p, v)) => Ok((p.to_owned(), v.to_owned())),
        None => Ok((
            coords.to_owned(),
            version
                .ok_or_else(|| eyre::eyre!("name the version: {coords}@VERSION or --version"))?,
        )),
    }
}

impl AppCmd {
    /// Runs without a node: `main` dispatches it before connecting.
    pub fn is_offline(&self) -> bool {
        matches!(
            self,
            Self::New(_)
                | Self::Build(_)
                | Self::Codegen { .. }
                | Self::Abi(_)
                | Self::Uses(_)
                | Self::Signer(_)
                | Self::Pack(_)
                | Self::Sign(_)
                | Self::Inspect { .. }
                | Self::Publish(_)
        ) || matches!(self, Self::Check(a) if !a.needs_node())
    }
}

/// The subcommands [`AppCmd::is_offline`] names.
pub async fn run_offline(cmd: AppCmd, home: &Home) -> eyre::Result<Value> {
    use crate::make;
    match cmd {
        AppCmd::New(a) => make::new(&a),
        AppCmd::Build(a) => make::build(&a),
        AppCmd::Codegen { args } => make::codegen(&args),
        AppCmd::Abi(c) => make::abi(&c),
        AppCmd::Uses(c) => make::uses::run(&c),
        AppCmd::Signer(c) => make::signer(&c),
        AppCmd::Pack(a) => make::pack(&a),
        AppCmd::Sign(a) => make::sign(&a),
        AppCmd::Inspect { mpk } => make::inspect(&mpk),
        AppCmd::Publish(a) => make::publish(&a, home).await,
        AppCmd::Check(a) => check::run(*a, None).await,
        _ => unreachable!("a node command"),
    }
}

pub async fn run(cmd: AppCmd, env: &Env) -> eyre::Result<Value> {
    Ok(match cmd {
        AppCmd::Package(cmd) => run_package(cmd, env).await?,
        AppCmd::Check(a) => check::run(*a, Some(env)).await?,
        AppCmd::Release(a) => release(&a, env).await?,
        AppCmd::New(_)
        | AppCmd::Build(_)
        | AppCmd::Codegen { .. }
        | AppCmd::Abi(_)
        | AppCmd::Uses(_)
        | AppCmd::Signer(_)
        | AppCmd::Pack(_)
        | AppCmd::Sign(_)
        | AppCmd::Inspect { .. }
        | AppCmd::Publish(_) => unreachable!("runs before connecting (main)"),
    })
}

/// `meroctl package` (and the same commands under `app`).
pub async fn run_package(cmd: PackageCmd, env: &Env) -> eyre::Result<Value> {
    let packages = env.client.packages();
    Ok(match cmd {
        PackageCmd::Install {
            coords,
            version,
            path,
            org,
            registry,
            registry_api_key,
        } => match (path, coords) {
            (Some(path), _) => install_file(env, &path, org.as_deref()).await?,
            (None, Some(coords)) => {
                let (package, version) = match (coords.contains('@'), version) {
                    (false, None) => {
                        let registry = Registry::new(&registry, registry_api_key, &env.home)?;
                        let version = latest(&registry, &coords).await?;
                        eprintln!(
                            "installing {coords}@{version}, the latest {} lists",
                            registry.url
                        );
                        (coords, version)
                    }
                    (_, version) => split_coords(&coords, version)?,
                };
                to_json(&packages.install(&package, &version, org.as_deref()).await?)?
            }
            (None, None) => unreachable!("clap requires one"),
        },
        PackageCmd::List => json!({ "packages": packages.list().await?.packages }),
        PackageCmd::Get { package } => to_json(&packages.get(&package).await?)?,
        PackageCmd::Versions { package } => {
            json!({ "package": package, "versions": packages.get(&package).await?.versions })
        }
        PackageCmd::GetLatestVersion { package } => {
            json!({ "package": package, "latest": packages.get(&package).await?.latest })
        }
        PackageCmd::Pipelines { package, version } => {
            let version = match version {
                Some(v) => v,
                None => packages.get(&package).await?.latest,
            };
            env.client.package_pipelines(&package, &version).await?
        }
        PackageCmd::Uninstall { package, version } => match version {
            Some(v) => to_json(&packages.version_uninstall(&package, &v).await?)?,
            None => to_json(&packages.uninstall(&package).await?)?,
        },
        PackageCmd::Watch(a) => {
            watch(env, &a, true).await?;
            return Ok(Value::Null);
        }
        PackageCmd::Origins(cmd) => origins(cmd, env).await?,
    })
}

/// The latest version of `package` the registry lists (not yanked).
async fn latest(registry: &Registry, package: &str) -> eyre::Result<String> {
    let release = registry
        .get(package, None)
        .await?
        .ok_or_else(|| registry.not_found(&format!("package {package}")))?;
    release["appVersion"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| eyre::eyre!("{} lists {package} with no version", registry.url))
}

/// `meroctl app release`: the version checked against the cluster's
/// releases of the package (and with `--publish` the registry's) first, then
/// `pack`, `publish` (with `--publish`) and the install, in that order; a
/// step that fails stops the rest.
pub async fn release(a: &crate::make::ReleaseArgs, env: &Env) -> eyre::Result<Value> {
    let package = a.package()?;
    if a.publish {
        // A Cloud org key (`--org`) does not read the registry: the saved
        // registry key, if any, does. A private package it cannot see is
        // left to the registry's own refusal (`version_not_allowed`).
        let key = a
            .org
            .is_none()
            .then(|| a.registry_api_key.clone())
            .flatten();
        let registry = Registry::new(&a.registry, key, &env.home)?;
        let published = registry.versions(&package).await?;
        if published
            .iter()
            .any(|b| b["appVersion"] == a.version.as_str())
        {
            eyre::bail!(
                "{} already holds {package}@{}: release a newer version",
                registry.url,
                a.version
            );
        }
    }
    let releases: Vec<String> = match env.client.packages().get(&package).await {
        Ok(p) => p.versions.into_iter().map(|v| v.version).collect(),
        Err(calimero_client::Error::Api { status: 404, .. }) => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    crate::make::check_newer(&package, &a.version, releases.iter().map(String::as_str))?;
    let packed = crate::make::pack(&a.pack_args())?;
    let mpk = packed["wrote"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| eyre::eyre!("pack answered no file: {packed}"))?;
    let published = if a.publish {
        crate::make::publish(&a.publish_args(mpk.clone()), &env.home).await?
    } else {
        Value::Null
    };
    // Published: the cluster installs it from its registry, as production
    // clusters only do. Otherwise uploaded, which development clusters take.
    let installed = if a.publish {
        to_json(
            &env.client
                .packages()
                .install(&package, &a.version, a.install_org.as_deref())
                .await?,
        )?
    } else {
        to_json(
            &env.client
                .packages()
                .upload(read_file(&mpk)?, a.install_org.as_deref())
                .await?,
        )?
    };
    Ok(json!({
        "package": package,
        "version": a.version,
        "mpk": mpk,
        "published": published,
        "installed": installed,
    }))
}

/// `meroctl package origins`. A change goes out with the manager's passkey
/// assertion when `--dev-passkey` names one; without it the request goes as
/// is, and the node's `412 CONFIRM_REQUIRED` is the answer (no passkey, no
/// change: the console makes it with the manager's own).
async fn origins(cmd: OriginsCmd, env: &Env) -> eyre::Result<Value> {
    let packages = env.client.packages();
    Ok(match cmd {
        OriginsCmd::List { package } => to_json(&packages.origins_list(&package).await?)?,
        OriginsCmd::Add {
            package,
            origin,
            rp_id,
            served,
            dev_passkey,
        } => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = packages
                .origins_add(
                    &package,
                    &origin,
                    rp_id.as_deref(),
                    served.then_some(true),
                    pk.get(),
                )
                .await;
            pk.save()?;
            to_json(&out?)?
        }
        OriginsCmd::Remove {
            package,
            origin,
            dev_passkey,
        } => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = packages.origins_remove(&package, &origin, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
    })
}

/// Push `path` to its instances (now, if `push_first`), and again on every
/// save unless `once`.
pub async fn watch(env: &Env, a: &WatchArgs, push_first: bool) -> eyre::Result<()> {
    let (path, once, interval_ms) = (a.path.as_path(), a.once, a.interval_ms);
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut seen = modified(path);
    let mut push_now = push_first;
    loop {
        if push_now {
            match push(env, a).await {
                Ok(out) => println!("{}", serde_json::to_string(&out)?),
                Err(e) if once => return Err(e),
                Err(e) => eprintln!("push failed: {e:#}"),
            }
            if once {
                return Ok(());
            }
        }
        push_now = true;
        // Wait for the next save, and for the writer to finish it.
        loop {
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            let now = modified(path);
            if now.is_some() && now != seen {
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                seen = modified(path);
                break;
            }
        }
        env.authenticate().await?;
    }
}

/// One push: the build, then each instance's update to it.
async fn push(env: &Env, a: &WatchArgs) -> eyre::Result<Value> {
    let (path, contexts, dev_passkey) = (&a.path, &a.contexts, &a.dev_passkey);
    let client = &env.client;
    let bytes = read_file(path)?;
    let (body, targets) = if path.extension().is_some_and(|e| e == "mpk") {
        let installed = client.packages().upload(bytes, a.org.as_deref()).await?;
        let targets = if contexts.is_empty() {
            let me = client.auth().me().await?.account_id;
            client
                .apps()
                .list(Some(&installed.package), None)
                .await?
                .apps
                .into_iter()
                .filter(|a| a.owner == me)
                .map(|a| a.app_id)
                .collect()
        } else {
            resolve_all(env, contexts).await?
        };
        let body = AppUpgradeRequest {
            version: Some(installed.version),
            ..AppUpgradeRequest::default()
        };
        (body, targets)
    } else {
        if contexts.is_empty() {
            eyre::bail!("a raw .wasm names the contexts it updates: --context ID");
        }
        let bundle = client.packages().bundle_put(bytes).await?.hash;
        let body = AppUpgradeRequest {
            bundle: Some(bundle),
            ..AppUpgradeRequest::default()
        };
        (body, resolve_all(env, contexts).await?)
    };
    let mut pk = load_dev_passkey(dev_passkey)?;
    let mut upgraded = Vec::new();
    let mut failed = serde_json::Map::new();
    for app in &targets {
        match client
            .apps()
            .upgrade(app, body.clone(), Some(&mut pk))
            .await
        {
            Ok(_) => upgraded.push(app.clone()),
            Err(e) => {
                let _prev = failed.insert(app.clone(), json!(e.to_string()));
            }
        }
    }
    save_passkey(dev_passkey, &pk)?;
    Ok(json!({ "pushed": body, "upgraded": upgraded, "failed": failed }))
}

async fn resolve_all(env: &Env, names: &[String]) -> eyre::Result<Vec<String>> {
    let mut out = Vec::with_capacity(names.len());
    for n in names {
        out.push(env.app(n).await?);
    }
    Ok(out)
}
