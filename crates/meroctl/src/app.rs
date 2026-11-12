//! `meroctl app`: signed packages, v1's applications. An application is
//! installed once on the cluster (verified on every node); contexts are its
//! running instances (`meroctl context create --package`). `meroctl package`
//! is the same command.

use std::path::{Path, PathBuf};
use std::time::Duration;

use calimero_client::types::AppUpgradeRequest;
use clap::Subcommand;
use serde_json::{json, Value};

use crate::env::{load_dev_passkey, read_file, save_passkey, to_json, DevPasskey, Env};

#[derive(Debug, Subcommand)]
pub enum AppCmd {
    /// Install an application: `PACKAGE@VERSION` (or `PACKAGE --version V`)
    /// from the node's registry, or a local file with `--path` (a signed
    /// `.mpk`; on a development cluster also a raw `.wasm`, which uploads a
    /// bundle).
    Install {
        #[arg(value_name = "PACKAGE[@VERSION]", required_unless_present = "path")]
        coords: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, conflicts_with = "coords")]
        path: Option<PathBuf>,
    },
    /// Installed applications and their versions.
    #[command(alias = "ls", alias = "list-packages")]
    List,
    /// One installed application.
    Get { package: String },
    /// An application's installed versions.
    #[command(alias = "list-versions")]
    Versions { package: String },
    /// An application's newest installed version.
    GetLatestVersion { package: String },
    /// Uninstall one version, or the whole application. Refused while a context runs it.
    Uninstall {
        package: String,
        #[arg(long)]
        version: Option<String>,
    },
    /// Push a build to its contexts now, then on every save (v1's `app
    /// watch`). A signed `.mpk` is installed and the contexts running its
    /// package are updated to its version; a raw `.wasm` (development
    /// clusters) updates the contexts named with `--context`.
    Watch {
        #[arg(long)]
        path: PathBuf,
        /// The contexts to update (default for a `.mpk`: this account's
        /// contexts of its package).
        #[arg(long = "context", short)]
        contexts: Vec<String>,
        /// The owner's passkey file that approves each update.
        #[arg(long)]
        dev_passkey: PathBuf,
        /// Push once and exit.
        #[arg(long)]
        once: bool,
        /// How often to look at the file, in milliseconds.
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
    },
    /// The web origins (a developer's own site, a hosting preview) whose
    /// pages may sign people in to this application, each under its own
    /// passkey rp-id. Its manager's: the publisher organization's
    /// `MANAGE_APPLICATION` holders, else whoever installed it first.
    #[command(subcommand)]
    Origins(OriginsCmd),
}

#[derive(Debug, Subcommand)]
pub enum OriginsCmd {
    /// The origins registered for an application, with their rp-ids and
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
        /// Have the cluster serve the application's frontend on this origin
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

/// Install a local file: a `.mpk` as a package, anything else as a raw bundle.
pub async fn install_file(env: &Env, path: &Path) -> eyre::Result<Value> {
    let bytes = read_file(path)?;
    if path.extension().is_some_and(|e| e == "mpk") {
        to_json(&env.client.packages().upload(bytes).await?)
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

pub async fn run(cmd: AppCmd, env: &Env) -> eyre::Result<Value> {
    let packages = env.client.packages();
    Ok(match cmd {
        AppCmd::Install {
            coords,
            version,
            path,
        } => match (path, coords) {
            (Some(path), _) => install_file(env, &path).await?,
            (None, Some(coords)) => {
                let (package, version) = split_coords(&coords, version)?;
                to_json(&packages.install(&package, &version).await?)?
            }
            (None, None) => unreachable!("clap requires one"),
        },
        AppCmd::List => json!({ "packages": packages.list().await?.packages }),
        AppCmd::Get { package } => to_json(&packages.get(&package).await?)?,
        AppCmd::Versions { package } => {
            json!({ "package": package, "versions": packages.get(&package).await?.versions })
        }
        AppCmd::GetLatestVersion { package } => {
            json!({ "package": package, "latest": packages.get(&package).await?.latest })
        }
        AppCmd::Uninstall { package, version } => match version {
            Some(v) => to_json(&packages.version_uninstall(&package, &v).await?)?,
            None => to_json(&packages.uninstall(&package).await?)?,
        },
        AppCmd::Watch {
            path,
            contexts,
            dev_passkey,
            once,
            interval_ms,
        } => {
            watch(env, &path, &contexts, &dev_passkey, once, interval_ms, true).await?;
            return Ok(Value::Null);
        }
        AppCmd::Origins(cmd) => origins(cmd, env).await?,
    })
}

/// `meroctl app origins`. A change goes out with the manager's passkey
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

/// Push `path` to its contexts (now, if `push_first`), and again on every
/// save unless `once`.
pub async fn watch(
    env: &Env,
    path: &Path,
    contexts: &[String],
    dev_passkey: &Path,
    once: bool,
    interval_ms: u64,
    push_first: bool,
) -> eyre::Result<()> {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut seen = modified(path);
    let mut push_now = push_first;
    loop {
        if push_now {
            match push(env, path, contexts, dev_passkey).await {
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

/// One push: the build, then each context's update to it.
async fn push(
    env: &Env,
    path: &Path,
    contexts: &[String],
    dev_passkey: &Path,
) -> eyre::Result<Value> {
    let client = &env.client;
    let bytes = read_file(path)?;
    let (body, targets) = if path.extension().is_some_and(|e| e == "mpk") {
        let installed = client.packages().upload(bytes).await?;
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
