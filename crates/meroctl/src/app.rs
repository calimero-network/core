//! `meroctl app`: signed packages, v1's applications. An application is
//! installed once on the cluster (verified on every node); contexts are its
//! running instances (`meroctl context create --package`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::env::{load_dev_passkey, read_file, save_passkey, Env};

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
}

/// Install a local file: a `.mpk` as a package, anything else as a raw bundle.
pub async fn install_file(env: &Env, path: &Path) -> eyre::Result<Value> {
    let bytes = read_file(path)?;
    if path.extension().is_some_and(|e| e == "mpk") {
        Ok(env.client.upload_package(bytes).await?)
    } else {
        Ok(json!({ "bundle": env.client.upload_bundle(bytes).await? }))
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

fn versions(package: &Value) -> Vec<Value> {
    package["versions"].as_array().cloned().unwrap_or_default()
}

pub async fn run(cmd: AppCmd, env: &Env) -> eyre::Result<Value> {
    Ok(match cmd {
        AppCmd::Install {
            coords,
            version,
            path,
        } => match (path, coords) {
            (Some(path), _) => install_file(env, &path).await?,
            (None, Some(coords)) => {
                let (package, version) = split_coords(&coords, version)?;
                env.client.install_package(&package, &version).await?
            }
            (None, None) => unreachable!("clap requires one"),
        },
        AppCmd::List => json!({ "packages": env.client.packages().await? }),
        AppCmd::Get { package } => env.client.package(&package).await?,
        AppCmd::Versions { package } => {
            json!({ "package": package, "versions": versions(&env.client.package(&package).await?) })
        }
        AppCmd::GetLatestVersion { package } => {
            let p = env.client.package(&package).await?;
            json!({ "package": package, "latest": p["latest"] })
        }
        AppCmd::Uninstall { package, version } => {
            env.client
                .uninstall_package(&package, version.as_deref())
                .await?
        }
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
        let installed = client.upload_package(bytes).await?;
        let package = installed["package"].as_str().unwrap_or_default().to_owned();
        let version = installed["version"].as_str().unwrap_or_default().to_owned();
        let targets = if contexts.is_empty() {
            let me = client.me().await?["accountId"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            client.get(&format!("apps?package={package}")).await?["apps"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter(|a| a["owner"].as_str() == Some(me.as_str()))
                .filter_map(|a| a["appId"].as_str().map(str::to_owned))
                .collect()
        } else {
            resolve_all(env, contexts).await?
        };
        (json!({ "version": version }), targets)
    } else {
        if contexts.is_empty() {
            eyre::bail!("a raw .wasm names the contexts it updates: --context ID");
        }
        let bundle = client.upload_bundle(bytes).await?;
        (
            json!({ "bundle": bundle }),
            resolve_all(env, contexts).await?,
        )
    };
    let mut pk = load_dev_passkey(dev_passkey)?;
    let mut upgraded = Vec::new();
    let mut failed = serde_json::Map::new();
    for app in &targets {
        match client.upgrade_app(app, body.clone(), &mut pk).await {
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
