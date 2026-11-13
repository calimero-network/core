//! `meroctl`: the command line for Calimero nodes (`merod`).
//!
//! v1's command tree over the node's API. Every request goes through the
//! attested, sealed client the SDKs use (`calimero-client`): the node's
//! quote is checked before anything is sent. Where v1 had an "application"
//! this has a signed package (`app`); a context is an app instance
//! (`context`); a namespace is an organization (`namespace`); a group is one
//! of its teams (`group`); an identity is an account with passkeys
//! (`account`). The operator's commands (`ops`) take `--ops-token`.

use std::path::PathBuf;

use calimero_client::types::ViewsRequestConsistency;
use calimero_identity::testing::SoftAuthenticator;
use clap::{Parser, Subcommand};
use eyre::bail;
use reqwest::Method;
use serde_json::{json, Value};

mod account;
mod app;
mod blob;
mod context;
mod dev;
mod env;
mod file;
mod group;
mod namespace;
mod node;
mod ops;
mod pipeline;
mod proxy;
mod run;

use env::{
    json_arg, load_dev_passkey, save_passkey, to_json, write_private, DevPasskey, Env, Home, Trust,
};

#[derive(Debug, Parser)]
#[command(
    name = "meroctl",
    // The release, not `CARGO_PKG_VERSION`: that is the workspace placeholder `0.0.0`.
    version = env!("CALIMERO_RELEASE_VERSION"),
    about = "Command line for Calimero nodes",
    after_help = "Examples:\n  \
        meroctl node add local http://127.0.0.1:2501 --allow-mock\n  \
        meroctl --node local login --dev-passkey ./dev-passkey.json\n  \
        meroctl --node local app install --path ./kv-store.mpk\n  \
        meroctl --node local context create --package com.example.kv --name notes\n  \
        meroctl --node local call --context notes set --args '{\"key\":\"a\",\"value\":\"1\"}'\n  \
        meroctl --node local --ops-token $TOKEN ops status\n  \
        meroctl --node local proxy --listen 127.0.0.1:2580"
)]
struct Cli {
    /// Directory for meroctl's saved nodes and sessions.
    #[arg(long, value_name = "PATH", env = "CALIMERO_HOME", default_value_os_t = default_home())]
    home: PathBuf,
    /// The node's URL (any node of the cluster).
    #[arg(long, value_name = "URL", env = "CALIMERO_API")]
    api: Option<String>,
    /// A node saved with `meroctl node add` (default: the one `node use` chose),
    /// or a node's URL (`http://` or `https://`, as `--api`).
    #[arg(long, value_name = "ALIAS|URL", conflicts_with = "api")]
    node: Option<String>,
    /// Trust mock quotes (development clusters such as `merod local`). A flag
    /// only, never read from the environment, so it cannot linger into a
    /// production call; `node add --allow-mock` remembers it for one node.
    #[arg(long)]
    allow_mock: bool,
    /// Trust a release's debug profiles (the host can read a debug image's memory).
    #[arg(long)]
    allow_debug_profiles: bool,
    /// Image profiles a real node's signed mero-tee release must list it under.
    #[arg(long, env = "CALIMERO_TEE_PROFILES", value_delimiter = ',')]
    tee_profiles: Vec<String>,
    /// The oldest mero-tee release to trust.
    #[arg(long, env = "CALIMERO_MIN_RELEASE")]
    min_release: Option<String>,
    /// Authenticate with this API key instead of a saved session.
    #[arg(long, env = "CALIMERO_API_KEY", hide_env_values = true)]
    api_key: Option<String>,
    /// The operator's token, for `ops` commands.
    #[arg(long, env = "CALIMERO_OPS_TOKEN", hide_env_values = true)]
    ops_token: Option<String>,
    /// A file holding the operator's Ed25519 key (32-byte seed, hex): `ops`
    /// commands sign the decisions governance gives the operator with it
    /// (the node is started with its public half, `merod --operator-key`).
    #[arg(long, env = "CALIMERO_OPERATOR_KEY_FILE")]
    operator_key_file: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

fn default_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".calimero")
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Saved nodes: add, remove, use, list.
    #[command(subcommand)]
    Node(node::NodeCmd),
    /// Sign in with a file passkey (development clusters only); signs up when the file is new.
    Login {
        #[arg(long)]
        dev_passkey: PathBuf,
        /// Account name, for a sign-up.
        #[arg(long, default_value = "developer")]
        name: String,
        /// The origin the passkey claims (must be one the cluster allows).
        #[arg(long, default_value = "http://localhost:3000")]
        origin: String,
    },
    /// Sign out: forget the saved session; `--all` ends every session of the account.
    Logout {
        #[arg(long)]
        all: bool,
    },
    /// Who the requests run as.
    Whoami,
    /// The node's attestation; with `--context`, every replica of that app
    /// verified, and the governance they attest.
    Attest {
        #[arg(long, short)]
        context: Option<String>,
    },
    /// Verify a release attestation (`merod attest`), made before the node
    /// holds any key: from `--file` with the `--nonce` it was made over, or
    /// fetched from the node (`GET /attestation/release`) over a fresh nonce.
    /// Judged like any quote (`--allow-mock`, `--tee-profiles`, `--min-release`).
    AttestRelease {
        #[arg(long, requires = "nonce")]
        file: Option<PathBuf>,
        /// The nonce the file's attestation was made over, hex.
        #[arg(long, requires = "file")]
        nonce: Option<String>,
    },
    /// Signed packages (v1's applications): install, list, versions,
    /// uninstall, watch, web origins. Also `meroctl package`.
    #[command(subcommand, alias = "package")]
    App(app::AppCmd),
    /// App instances (v1's contexts): create, list, get, update, delete, aliases, access.
    #[command(subcommand)]
    Context(context::ContextCmd),
    /// Call a method on a context (a write), or `--view` to read.
    Call {
        /// The context (an id or an alias).
        #[arg(long, short)]
        context: String,
        method: String,
        /// JSON arguments.
        #[arg(long, default_value = "{}")]
        args: String,
        /// Read only (a view): no write, answered from `--min-index` or the leader.
        #[arg(long)]
        view: bool,
        /// A view from any replica at least this fresh, instead of the leader.
        #[arg(long, requires = "view")]
        min_index: Option<u64>,
        /// Retries with the same key are answered once.
        #[arg(long)]
        idempotency_key: Option<String>,
        /// For a method that needs a passkey confirmation per call (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// A context's blobs: list, upload, download, info, delete.
    #[command(subcommand)]
    Blob(blob::BlobCmd),
    /// A context's sealed files: upload.
    #[command(subcommand)]
    File(file::FileCmd),
    /// A context's pipeline runs: run, get, cancel, runs.
    #[command(subcommand)]
    Pipeline(pipeline::PipelineCmd),
    /// A context's compute runs: logs.
    #[command(subcommand)]
    Run(run::RunCmd),
    /// Organizations (v1's namespaces): members, invites, groups, metadata, approvals.
    #[command(subcommand, alias = "ns")]
    Namespace(namespace::NamespaceCmd),
    /// An organization's teams (v1's groups): members, metadata, reparent, upgrades.
    #[command(subcommand)]
    Group(group::GroupCmd),
    /// This account: passkeys (devices), API keys, usage, approvals.
    #[command(subcommand)]
    Account(account::AccountCmd),
    /// Development workflow: install, create and keep a build pushed.
    #[command(subcommand)]
    Dev(dev::DevCmd),
    /// The node's health and the cluster's leader.
    #[command(subcommand)]
    Network(NetworkCmd),
    /// The operator's commands (need `--ops-token`): nodes, releases,
    /// packages, upgrades, quotas, backups, metrics.
    #[command(subcommand)]
    Ops(ops::OpsCmd),
    /// Serve the node's API as plain HTTP on loopback: every request is sent
    /// through the attested, sealed channel with your session (or the
    /// request's own `Authorization`); `GET /openapi.json` is the API's
    /// document. For curl, scripts and OpenAPI clients.
    Proxy(proxy::ProxyArgs),
    /// Any API route: METHOD is get, post, put or delete; BODY is JSON.
    Raw {
        method: String,
        path: String,
        body: Option<String>,
        /// Answer a `412 CONFIRM_REQUIRED` on a post or delete with this
        /// passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum NetworkCmd {
    /// This node, its release, and who leads the system group.
    Status,
}

/// Print a command's answer as JSON. `null` is what commands that wrote
/// their own output (streams, downloads, metrics) answer: nothing more to say.
fn print(v: &Value) -> eyre::Result<()> {
    if !v.is_null() {
        println!("{}", serde_json::to_string_pretty(v)?);
    }
    Ok(())
}

fn trust(cli: &Cli, saved_mock: bool) -> Trust {
    Trust {
        allow_mock: cli.allow_mock || saved_mock,
        allow_debug_profiles: cli.allow_debug_profiles,
        tee_profiles: cli.tee_profiles.clone(),
        min_release: cli.min_release.clone(),
    }
}

fn hex32(s: &str) -> eyre::Result<[u8; 32]> {
    hex::decode(s)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| eyre::eyre!("--nonce must be 32 bytes hex"))
}

/// Verify a release attestation and answer what it proved.
async fn verified_release(
    doc: &calimero_client::attest::release::ReleaseAttestation,
    nonce: &[u8; 32],
    trust: &Trust,
) -> eyre::Result<Value> {
    let v = calimero_client::attest::release::verify_release(doc, nonce, &trust.policy()).await?;
    Ok(json!({
        "verified": true,
        "mock": v.measurements.is_none(),
        "claim": v.claim,
        "measurements": v.measurements,
        "profile": v.profile,
    }))
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Cli::parse();
    let home = Home {
        dir: cli.home.clone(),
    };
    if let Cmd::Node(cmd) = &cli.cmd {
        return print(&node::run(cmd, &home)?);
    }

    if let Cmd::AttestRelease {
        file: Some(file),
        nonce,
    } = &cli.cmd
    {
        let doc = serde_json::from_slice(&std::fs::read(file)?)?;
        let nonce = hex32(nonce.as_deref().unwrap_or_default())?;
        return print(&verified_release(&doc, &nonce, &trust(&cli, false)).await?);
    }

    let (url, saved_mock) = match (&cli.api, &cli.node) {
        (Some(url), _) => (url.clone(), false),
        (None, Some(url)) if url.starts_with("http://") || url.starts_with("https://") => {
            (url.clone(), false)
        }
        (None, name) => {
            let nodes = home.nodes();
            let name = name.clone().or(nodes.default.clone()).ok_or_else(|| {
                eyre::eyre!(
                    "which node? --api URL, --node ALIAS, or save one: meroctl node add NAME URL"
                )
            })?;
            let n = nodes
                .nodes
                .get(&name)
                .ok_or_else(|| eyre::eyre!("no saved node {name}: meroctl node add {name} URL"))?;
            (n.url.clone(), n.allow_mock)
        }
    };
    let trust = trust(&cli, saved_mock);
    if let Cmd::AttestRelease { file: None, .. } = &cli.cmd {
        let nonce = calimero_identity::random::<32>();
        let base = url::Url::parse(&url)?;
        let doc =
            calimero_client::attest::release::fetch_release_attestation(&base, &nonce).await?;
        return print(&verified_release(&doc, &nonce, &trust).await?);
    }
    if let Cmd::Proxy(args) = cli.cmd {
        return proxy::run(args, home, url, trust.policy(), cli.api_key.clone()).await;
    }
    let mut env = Env::connect(home, url, trust, cli.api_key.clone(), cli.ops_token.clone())?;
    env.operator_key_file.clone_from(&cli.operator_key_file);

    if let Cmd::Ops(cmd) = cli.cmd {
        return print(&ops::run(cmd, &env).await?);
    }
    match &cli.cmd {
        Cmd::Login {
            dev_passkey,
            name,
            origin,
        } => {
            if !env.trust.allow_mock {
                bail!("a file passkey is for development clusters (--allow-mock); on a real cluster use an API key from the console");
            }
            let session = if dev_passkey.exists() {
                let mut pk = load_dev_passkey(dev_passkey)?;
                let s = env.client.sign_in(&mut pk).await;
                save_passkey(dev_passkey, &pk)?;
                s?
            } else {
                let mut pk = SoftAuthenticator::new("localhost", origin);
                let s = env.client.sign_up(&mut pk, name).await;
                write_private(dev_passkey, &pk.to_json())?;
                s?
            };
            env.home.save_session(&env.url, Some(&session))?;
            return print(&json!({ "accountId": session.account_id, "passkey": dev_passkey }));
        }
        Cmd::Logout { all } => {
            if *all {
                env.authenticate().await?;
                let _ = env.client.auth().logout_all().await?;
            }
            env.home.save_session(&env.url, None)?;
            return Ok(());
        }
        _ => {}
    }

    env.authenticate().await?;
    let out: Value = match cli.cmd {
        Cmd::Whoami => to_json(&env.client.auth().me().await?)?,
        Cmd::Attest { context: None } => {
            // For display: the channel attested the node already, under the policy.
            let nonce = hex::encode([7_u8; 32]);
            to_json(
                &env.client
                    .public()
                    .attestation(&nonce, None, None, None)
                    .await?,
            )?
        }
        Cmd::Attest {
            context: Some(context),
        } => {
            let app = env.app(&context).await?;
            let proven = env.client.verify_app_cluster(&app).await?;
            json!({
                "app": app,
                "receiptKeys": proven.keys.iter()
                    .map(|(node, k)| (node.to_string(), json!(hex::encode(k))))
                    .collect::<serde_json::Map<_, _>>(),
                "governance": proven.governance,
            })
        }
        Cmd::App(cmd) => app::run(cmd, &env).await?,
        Cmd::Context(cmd) => context::run(cmd, &env).await?,
        Cmd::Call {
            context,
            method,
            args,
            view,
            min_index,
            idempotency_key,
            dev_passkey,
        } => {
            let app = env.app(&context).await?;
            let args = Some(json_arg(&args)?);
            if view {
                let (consistency, min_index) = match min_index {
                    Some(i) => (ViewsRequestConsistency::Stale, Some(i)),
                    None => (ViewsRequestConsistency::Strong, None),
                };
                to_json(
                    &env.client
                        .apps()
                        .view(&app, &method, args, Some(consistency), min_index)
                        .await?,
                )?
            } else {
                let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
                let out = env
                    .client
                    .apps()
                    .call(&app, &method, args, idempotency_key.as_deref(), pk.get())
                    .await;
                pk.save()?;
                to_json(&out?)?
            }
        }
        Cmd::Blob(cmd) => blob::run(cmd, &env).await?,
        Cmd::File(cmd) => file::run(cmd, &env).await?,
        Cmd::Pipeline(cmd) => pipeline::run(cmd, &env).await?,
        Cmd::Run(cmd) => run::run(cmd, &env).await?,
        Cmd::Namespace(cmd) => namespace::run(cmd, &env).await?,
        Cmd::Group(cmd) => group::run(cmd, &env).await?,
        Cmd::Account(cmd) => account::run(cmd, &env).await?,
        Cmd::Dev(cmd) => return dev::run(cmd, &env).await,
        Cmd::Network(NetworkCmd::Status) => to_json(&env.client.public().health().await?)?,
        Cmd::Raw {
            method,
            path,
            body,
            dev_passkey,
        } => {
            let path = path.trim_start_matches('/');
            let body = body.as_deref().map(json_arg).transpose()?;
            let dev_passkey = dev_passkey.as_deref();
            match method.to_ascii_lowercase().as_str() {
                "get" => env.raw(Method::GET, path, None, None).await?,
                // A passkey's `confirm` goes into a JSON body: an empty one when none is given.
                "delete" => {
                    let body = body.or_else(|| dev_passkey.map(|_| json!({})));
                    env.raw(Method::DELETE, path, body, dev_passkey).await?
                }
                "post" => {
                    let body = body.unwrap_or_else(|| json!({}));
                    env.raw(Method::POST, path, Some(body), dev_passkey).await?
                }
                "put" => {
                    let body = body.unwrap_or_else(|| json!({}));
                    env.raw(Method::PUT, path, Some(body), None).await?
                }
                other => bail!("unknown method {other}: use get, post, put or delete"),
            }
        }
        Cmd::Node(_)
        | Cmd::Ops(_)
        | Cmd::Login { .. }
        | Cmd::Logout { .. }
        | Cmd::Proxy(_)
        | Cmd::AttestRelease { .. } => {
            unreachable!("handled above")
        }
    };
    print(&out)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Cli;

    /// `package` is `app`: `meroctl package origins ...` reaches the same commands.
    #[test]
    fn package_is_app() {
        for args in [
            &["meroctl", "package", "origins", "list", "com.example.kv"][..],
            &[
                "meroctl",
                "package",
                "origins",
                "add",
                "com.example.kv",
                "https://kv.example.com",
                "--rp-id",
                "example.com",
            ][..],
            &[
                "meroctl",
                "app",
                "origins",
                "add",
                "com.example.kv",
                "https://kv.example.com",
                "--served",
            ][..],
            &[
                "meroctl",
                "app",
                "origins",
                "rm",
                "com.example.kv",
                "https://kv.example.com",
                "--dev-passkey",
                "pk.json",
            ][..],
        ] {
            let parsed = Cli::try_parse_from(args);
            assert!(parsed.is_ok(), "{args:?}: {}", parsed.unwrap_err());
        }
    }

    /// A passkey's credential id is base64url and may start with `-`; clap
    /// must take it as the value, not as a flag.
    #[test]
    fn a_credential_id_starting_with_a_dash_is_a_value() {
        for args in [
            &[
                "meroctl",
                "account",
                "device",
                "label",
                "-7qCMOBQeBL02g",
                "laptop",
            ][..],
            &["meroctl", "account", "device", "revoke", "-7qCMOBQeBL02g"][..],
        ] {
            let parsed = Cli::try_parse_from(args);
            assert!(parsed.is_ok(), "{args:?}: {}", parsed.unwrap_err());
        }
    }
}
