//! `meroctl app`: signed packages, v1's applications. An application is
//! installed once on the cluster (verified on every node); contexts are its
//! running instances (`meroctl context create --package`). `meroctl package`
//! is the same command.

use std::path::{Path, PathBuf};
use std::time::Duration;

use calimero_client::package_origins_path;
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
        AppCmd::List => {
            json!({ "packages": serde_json::to_value(env.client.packages().list().await?)?["packages"] })
        }
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
        AppCmd::Origins(cmd) => origins(cmd, env).await?,
    })
}

/// `meroctl app origins`. A change goes out with the manager's passkey
/// assertion when `--dev-passkey` names one; without it the request goes as
/// is, and the node's `412 CONFIRM_REQUIRED` is the answer (no passkey, no
/// change: the console makes it with the manager's own).
async fn origins(cmd: OriginsCmd, env: &Env) -> eyre::Result<Value> {
    let client = &env.client;
    Ok(match cmd {
        OriginsCmd::List { package } => client.package_origins(&package).await?,
        OriginsCmd::Add {
            package,
            origin,
            rp_id,
            served,
            dev_passkey: Some(file),
        } => {
            let mut pk = load_dev_passkey(&file)?;
            let out = client
                .post_confirmed(
                    &package_origins_path(&package),
                    add_origin_body(&origin, rp_id.as_deref(), served),
                    &mut pk,
                )
                .await;
            save_passkey(&file, &pk)?;
            out?
        }
        OriginsCmd::Add {
            package,
            origin,
            rp_id,
            served,
            dev_passkey: None,
        } => {
            client
                .post(
                    &package_origins_path(&package),
                    add_origin_body(&origin, rp_id.as_deref(), served),
                )
                .await?
        }
        OriginsCmd::Remove {
            package,
            origin,
            dev_passkey: Some(file),
        } => {
            let mut pk = load_dev_passkey(&file)?;
            let out = client
                .remove_package_origin(&package, &origin, &mut pk)
                .await;
            save_passkey(&file, &pk)?;
            out?
        }
        OriginsCmd::Remove {
            package,
            origin,
            dev_passkey: None,
        } => {
            client
                .delete_with_body(&package_origins_path(&package), json!({ "origin": origin }))
                .await?
        }
    })
}

/// What `POST /package-origins/{package}` takes: the rp-id only when named
/// (the node resolves the origin's host otherwise), and `served` only when
/// set, so a plain add is the body a node from before served origins takes.
fn add_origin_body(origin: &str, rp_id: Option<&str>, served: bool) -> Value {
    let mut body = json!({ "origin": origin });
    if let Some(rp_id) = rp_id {
        body["rpId"] = json!(rp_id);
    }
    if served {
        body["served"] = json!(true);
    }
    body
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::add_origin_body;

    #[test]
    fn an_origin_names_its_rp_id_only_when_given() {
        assert_eq!(
            add_origin_body("https://kv.example.com", None, false),
            json!({ "origin": "https://kv.example.com" })
        );
        assert_eq!(
            add_origin_body("https://kv.example.com", Some("example.com"), false),
            json!({ "origin": "https://kv.example.com", "rpId": "example.com" })
        );
    }

    /// `--served` sends `served: true`, and nothing about it otherwise: the
    /// node's consent names `served` only for a served origin.
    #[test]
    fn a_served_origin_says_so_and_a_plain_one_does_not() {
        assert_eq!(
            add_origin_body("https://kv.example.com", None, true),
            json!({ "origin": "https://kv.example.com", "served": true })
        );
        assert_eq!(
            add_origin_body("https://kv.example.com", Some("example.com"), true),
            json!({ "origin": "https://kv.example.com", "rpId": "example.com", "served": true })
        );
        assert!(add_origin_body("https://kv.example.com", None, false)
            .get("served")
            .is_none());
    }
}
