//! `meroctl`: the command line for Calimero nodes (`merod`).
//!
//! v1's command tree over the node's API. Every request goes through the
//! attested, sealed client the SDKs use (`calimero-client`): the node's
//! quote is checked before anything is sent. The names are the glossary's:
//! `app` makes and ships an app's signed package (v1's application),
//! `package` handles the ones installed on the cluster; a context is an app
//! instance (`instance`); a namespace is an organization (`org`); a group is
//! one of its teams (`team`); an identity is an account with passkeys
//! (`account`). The operator's commands (`ops`) take `--ops-token`.
//!
//! Spec: [meroctl CLI reference](https://calimero-network.github.io/core/operate/meroctl/).
//! Crate guide: [`crates/meroctl/AGENTS.md`](https://github.com/calimero-network/core/blob/master/crates/meroctl/AGENTS.md).

use std::path::PathBuf;

use calimero_client::types::ViewsRequestConsistency;
use calimero_identity::testing::SoftAuthenticator;
use clap::{Parser, Subcommand};
use eyre::{bail, eyre};
use reqwest::Method;
use serde_json::{json, Value};

mod account;
mod app;
mod blob;
mod cloud;
mod context;
mod deployment;
mod dev;
mod env;
mod file;
mod group;
mod local;
mod make;
mod namespace;
mod node;
mod ops;
mod pipeline;
mod proxy;
mod registry;
mod run;

use env::{
    json_arg, load_dev_passkey, save_passkey, to_json, write_private, DevPasskey, Env, Home, Trust,
};

#[derive(Debug, Parser)]
#[command(
    name = "meroctl",
    // The release, not `CARGO_PKG_VERSION`: that is the workspace placeholder `0.0.0`.
    version = concat!(
        env!("CALIMERO_RELEASE_VERSION"),
        " (",
        env!("CALIMERO_BUILD_DESCRIBE"),
        ", ",
        env!("CALIMERO_BUILD_COMMIT"),
        ")"
    ),
    about = "Command line for Calimero nodes",
    after_help = "Examples:\n  \
        meroctl app new my-app\n  \
        meroctl node add local http://127.0.0.1:2501 --allow-mock\n  \
        meroctl --node local login --dev-passkey ./dev-passkey.json\n  \
        meroctl --node local package install --path ./kv-store.mpk\n  \
        meroctl --node local instance create --package com.example.kv --name notes\n  \
        meroctl --node local call --instance notes set --args '{\"key\":\"a\",\"value\":\"1\"}'\n  \
        meroctl --node local app release --version 1.4.0\n  \
        meroctl --node local deployment offer acme-apps notes 1.4.0\n  \
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

/// A subcommand still parsed under an old spelling, but out of the help.
fn hidden(cmd: clap::Command) -> clap::Command {
    cmd.hide(true)
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
    /// Attestation: the node's (the default), an app's replicas, a release's.
    Attest {
        #[command(subcommand)]
        cmd: Option<AttestCmd>,
    },
    /// Make an app and ship its signed package (v1's application): new,
    /// build, codegen, abi, uses, signer, pack, sign, inspect, publish,
    /// check, release. No node, but for `release` and `check --against VERSION`.
    #[command(
        subcommand,
        mut_subcommand("install", hidden),
        mut_subcommand("list", hidden),
        mut_subcommand("get", hidden),
        mut_subcommand("versions", hidden),
        mut_subcommand("get-latest-version", hidden),
        mut_subcommand("pipelines", hidden),
        mut_subcommand("uninstall", hidden),
        mut_subcommand("watch", hidden),
        mut_subcommand("origins", hidden)
    )]
    App(app::AppCmd),
    /// The packages installed on the cluster: install, list, get, versions,
    /// pipelines, uninstall, watch (push a build), origins.
    #[command(subcommand)]
    Package(app::PackageCmd),
    /// App instances (v1's contexts): create, list, get, update, delete, aliases, access.
    #[command(subcommand, name = "instance")]
    Context(context::ContextCmd),
    /// Call a method on an instance (a write), or `--view` to read.
    Call {
        /// The app instance (an id or an alias).
        #[arg(long = "instance", short = 'i', value_name = "INSTANCE")]
        #[arg(value_name = "INSTANCE")]
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
        /// Watch a subscribable view: print it now, then its new answer each time it
        /// changes, one JSON line each.
        #[arg(long, requires = "view", conflicts_with = "min_index")]
        watch: bool,
        /// With `--watch`: stop after this many updates.
        #[arg(long, requires = "watch")]
        limit: Option<usize>,
        /// Retries with the same key are answered once.
        #[arg(long)]
        idempotency_key: Option<String>,
        /// For a method that needs a passkey confirmation per call (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Deployments: offer your app's releases to other organizations, set
    /// the secret values their instances use, see the customer organizations.
    #[command(subcommand)]
    Deployment(deployment::DeploymentCmd),
    /// An instance's blobs: list, upload, download, info, delete.
    #[command(subcommand)]
    Blob(blob::BlobCmd),
    /// An instance's sealed files: upload.
    #[command(subcommand)]
    File(file::FileCmd),
    /// Calimero Cloud: sign in, organizations, clusters, `use` one as the
    /// node; and a cluster's compute (pipelines, run logs).
    #[command(subcommand)]
    Cloud(cloud::CloudCmd),
    /// The app registry `app publish` publishes to: list, search, a
    /// package's versions, one release; `login` saves an API key for private
    /// packages and `--mine`. No node.
    Registry(registry::RegistryArgs),
    /// Organizations (v1's namespaces): members, invites, teams, metadata, approvals.
    #[command(subcommand, name = "org")]
    Namespace(namespace::NamespaceCmd),
    /// An organization's teams (v1's groups): members, metadata, reparent, upgrades.
    #[command(subcommand, name = "team")]
    Group(group::GroupCmd),
    /// This account: passkeys (devices), API keys, usage, approvals.
    #[command(subcommand)]
    Account(account::AccountCmd),
    /// A development cluster on this machine (`merod local` in the release's
    /// image, or from PATH with `--local`): up, down, status; and the loop
    /// around it: dev, test, compute run.
    #[command(subcommand)]
    Local(local::LocalCmd),
    /// `meroctl local dev`'s predecessor: install a build, create an instance
    /// from it and keep it pushed.
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
        /// Answer a `412 CONFIRM_REQUIRED` on a post, put or delete with this
        /// passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum AttestCmd {
    /// The node's attestation, as the channel verified it.
    Node,
    /// Every replica of an app verified, and the governance they attest.
    App {
        #[arg(value_name = "INSTANCE")]
        context: String,
    },
    /// Verify a release attestation (`merod attest`), made before the node
    /// holds any key: from `--file` with the `--nonce` it was made over, or
    /// fetched from the node (`GET /attestation/release`) over a fresh nonce.
    /// Judged like any quote (`--allow-mock`, `--tee-profiles`, `--min-release`).
    Release {
        #[arg(long, requires = "nonce")]
        file: Option<PathBuf>,
        /// The nonce the file's attestation was made over, hex.
        #[arg(long, requires = "file")]
        nonce: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum NetworkCmd {
    /// This node, its release, and who leads the system group.
    Status,
}

/// `call --view --watch`: the view now (`{ index, result }`), then its new answer each
/// time it changes (`{ index, result }`, `{ index, error }` for an app error, or `{ notice,
/// index, result }` read again when the answer was too large to push), one JSON line each;
/// `{ "ended": true, "reason" }` when the node drops the watch.
async fn watch_view(
    client: &calimero_client::Client,
    app: &str,
    method: &str,
    args: Value,
    limit: Option<usize>,
) -> eyre::Result<()> {
    use calimero_client::WatchNotice;
    use futures_util::StreamExt;

    let mut watch = client.watch(app, method, args.clone()).await?;
    println!(
        "{}",
        json!({ "index": watch.index, "result": watch.answer })
    );
    let mut reads = 0;
    while limit.is_none_or(|l| reads < l) {
        let line = match watch.notices.next().await {
            Some(WatchNotice::Answer { index, answer }) => {
                json!({ "index": index, "result": answer })
            }
            Some(WatchNotice::Failed { index, error }) => json!({ "index": index, "error": error }),
            Some(WatchNotice::Changed { index }) => {
                let v = client
                    .apps()
                    .view(
                        app,
                        method,
                        Some(args.clone()),
                        Some(ViewsRequestConsistency::Stale),
                        Some(index),
                    )
                    .await?;
                json!({ "notice": index, "index": v.index, "result": v.result })
            }
            Some(WatchNotice::Ended { reason }) => {
                println!("{}", json!({ "ended": true, "reason": reason }));
                break;
            }
            None => {
                println!("{}", json!({ "ended": true }));
                break;
            }
        };
        println!("{line}");
        reads += 1;
    }
    Ok(())
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

/// The old spellings an invocation used, each with the one that replaced it.
/// They no longer parse; when a command line is refused, `main` names the
/// replacement of each one it used under clap's error.
fn old_spellings(args: &[String]) -> Vec<(&'static str, &'static str)> {
    use clap::CommandFactory as _;
    // meroctl's own options that take a value, so `--node group` is a value.
    let cmd = Cli::command();
    let takes_value: Vec<String> = cmd
        .get_arguments()
        .filter(|a| a.get_action().takes_values())
        .filter_map(|a| a.get_long().map(|l| format!("--{l}")))
        .collect();
    let mut words = Vec::new();
    let mut i = 1;
    while i < args.len() && words.len() < 2 {
        let a = args[i].as_str();
        if a.starts_with('-') {
            if takes_value.iter().any(|l| l == a) && words.is_empty() {
                i += 1;
            }
        } else {
            words.push(a);
        }
        i += 1;
    }
    let (top, sub) = (words.first().copied(), words.get(1).copied());
    let mut found = Vec::new();
    match top {
        Some("context") => found.push(("meroctl context", "meroctl instance")),
        Some("namespace" | "ns") => found.push(("meroctl namespace", "meroctl org")),
        Some("group") => found.push(("meroctl group", "meroctl team")),
        Some("attest-release") => found.push(("meroctl attest-release", "meroctl attest release")),
        Some("pipeline") => found.push(("meroctl pipeline", "meroctl cloud compute pipeline")),
        Some("run") => found.push(("meroctl run logs", "meroctl cloud compute logs")),
        _ => {}
    }
    match (top, sub) {
        (Some("namespace" | "ns" | "org"), Some("groups")) => found.push(("groups", "teams")),
        (Some("namespace" | "ns" | "org"), Some("create-group")) => {
            found.push(("create-group", "create-team"));
        }
        (Some("group" | "team"), Some("subgroups")) => found.push(("subgroups", "subteams")),
        (Some("ops"), Some("upgrade-namespace")) => {
            found.push(("upgrade-namespace", "upgrade-org"))
        }
        (Some("dev"), Some("new")) => found.push(("meroctl dev new", "meroctl app new")),
        // `package` made and shipped packages too while it was `app`'s alias.
        (
            Some("package"),
            Some(
                "new" | "build" | "codegen" | "abi" | "uses" | "signer" | "pack" | "sign"
                | "inspect" | "publish" | "check" | "release",
            ),
        ) => found.push(("meroctl package", "meroctl app")),
        _ => {}
    }
    let has = |flag: &str| {
        args.iter()
            .skip(1)
            .any(|a| a == flag || a.starts_with(&format!("{flag}=")))
    };
    if top == Some("attest") && has("--context") {
        found.push(("meroctl attest --context", "meroctl attest app"));
    } else if has("--context") {
        found.push(("--context", "--instance"));
    }
    if args.iter().skip(1).any(|a| a == "-c") {
        found.push(("-c", "-i"));
    }
    if has("--namespace") {
        found.push(("--namespace", "--org"));
    }
    if has("--group") {
        found.push(("--group", "--team"));
    }
    found
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let argv: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let cli = match Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(e) => {
            let renamed = old_spellings(&argv);
            if e.use_stderr() && !renamed.is_empty() {
                let _ = e.print();
                for (old, new) in renamed {
                    eprintln!("note: `{old}` was renamed `{new}`");
                }
                std::process::exit(e.exit_code());
            }
            e.exit()
        }
    };
    let home = Home {
        dir: cli.home.clone(),
    };
    if let Cmd::Node(cmd) = &cli.cmd {
        return print(&node::run(cmd, &home)?);
    }
    if matches!(&cli.cmd, Cmd::Cloud(c) if !c.needs_node()) {
        let Cmd::Cloud(cmd) = cli.cmd else {
            unreachable!()
        };
        return print(&cloud::run(cmd, &home).await?);
    }
    if matches!(cli.cmd, Cmd::Registry(_)) {
        let Cmd::Registry(args) = cli.cmd else {
            unreachable!()
        };
        return print(&registry::run(args, &home).await?);
    }
    if matches!(cli.cmd, Cmd::Local(_)) {
        let Cmd::Local(cmd) = cli.cmd else {
            unreachable!()
        };
        return print(&local::run(cmd, home).await?);
    }
    if matches!(&cli.cmd, Cmd::App(c) if c.is_offline()) {
        let Cmd::App(cmd) = cli.cmd else {
            unreachable!()
        };
        return print(&app::run_offline(cmd, &home).await?);
    }

    if let Cmd::Attest {
        cmd: Some(AttestCmd::Release {
            file: Some(file),
            nonce,
        }),
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
    if let Cmd::Attest {
        cmd: Some(AttestCmd::Release { file: None, .. }),
    } = &cli.cmd
    {
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
                // The cluster answers the same for a passkey removed from its
                // account and for a cluster that started over (`local up
                // --fresh`), so this never signs up on its own: a removed
                // passkey stays refused. It says what to do for a new cluster.
                s.map_err(|e| {
                    if e.code() == Some("PASSKEY_UNKNOWN_HERE") {
                        eyre!(
                            "{e}\nIf this cluster started over (`meroctl local up --fresh`), it has no account for {}: \
                             sign up again with a new passkey file (delete it, or pass another path to --dev-passkey).",
                            dev_passkey.display()
                        )
                    } else {
                        e.into()
                    }
                })?
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
        Cmd::Attest {
            cmd: None | Some(AttestCmd::Node),
        } => {
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
            cmd: Some(AttestCmd::App { context }),
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
        Cmd::Package(cmd) => app::run_package(cmd, &env).await?,
        Cmd::Context(cmd) => context::run(cmd, &env).await?,
        Cmd::Call {
            context,
            method,
            args,
            view,
            min_index,
            watch,
            limit,
            idempotency_key,
            dev_passkey,
        } => {
            let app = env.app(&context).await?;
            let args = json_arg(&args)?;
            if watch {
                return watch_view(&env.client, &app, &method, args, limit).await;
            }
            let args = Some(args);
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
        Cmd::Deployment(cmd) => deployment::run(cmd, &env).await?,
        Cmd::Blob(cmd) => blob::run(cmd, &env).await?,
        Cmd::File(cmd) => file::run(cmd, &env).await?,
        Cmd::Cloud(cloud::CloudCmd::Compute(cmd)) => match cmd {
            cloud::ComputeCmd::Pipeline(cmd) => pipeline::run(cmd, &env).await?,
            cloud::ComputeCmd::Logs { app, run, attempt } => {
                run::run(run::RunCmd::Logs { app, run, attempt }, &env).await?
            }
        },
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
                // A step-up `PUT` (an app's or a deployment's secret) signs with the dev passkey.
                "put" => {
                    let body = body.unwrap_or_else(|| json!({}));
                    env.raw(Method::PUT, path, Some(body), dev_passkey).await?
                }
                other => bail!("unknown method {other}: use get, post, put or delete"),
            }
        }
        Cmd::Node(_)
        | Cmd::Local(_)
        | Cmd::Cloud(_)
        | Cmd::Registry(_)
        | Cmd::Ops(_)
        | Cmd::Login { .. }
        | Cmd::Logout { .. }
        | Cmd::Proxy(_)
        | Cmd::Attest {
            cmd: Some(AttestCmd::Release { .. }),
        } => {
            unreachable!("handled above")
        }
    };
    print(&out)
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::Cli;

    /// Clap's own checks over the whole tree (unique names, valid defaults):
    /// otherwise they fail only when that subcommand is run.
    #[test]
    fn the_command_tree_is_consistent() {
        Cli::command().debug_assert();
    }

    /// No help a user can see (command names, flags, value names, text, the
    /// examples) uses a word the glossary retired: an app instance is not a
    /// context, an organization not a namespace, a team not a group (a Raft
    /// group is), a signed package not an application. Spec:
    /// [Glossary](https://calimero-network.github.io/core/protocol/glossary/#retired-words)
    #[test]
    fn help_uses_the_glossary_words() {
        fn walk(cmd: &mut clap::Command, path: &str, found: &mut Vec<String>) {
            let help = cmd.render_long_help().to_string();
            for line in help.lines() {
                let l = line.to_lowercase();
                let old_instance = l.contains("context") && !l.contains("v1's contexts");
                // `MANAGE_APPLICATION` is the capability's name in the API.
                let old_package = l.replace("manage_application", "").contains("application")
                    && !l.contains("v1's application")
                    && !l.contains("application/");
                // `org` and `team`, not v1's `namespace` and `group` (a Raft
                // group is another thing, named so).
                let old_org = !l.contains("v1's")
                    && (l.contains("namespace")
                        || (l.contains("group")
                            && !l.contains("raft group")
                            && !l.contains("system group")));
                if old_instance || old_org || old_package {
                    found.push(format!("{path}: {}", line.trim()));
                }
            }
            for sub in cmd.get_subcommands_mut() {
                if sub.is_hide_set() {
                    continue;
                }
                let next = format!("{path} {}", sub.get_name());
                walk(sub, &next, found);
            }
        }
        let mut cmd = Cli::command();
        cmd.build();
        let mut found = Vec::new();
        walk(&mut cmd, "meroctl", &mut found);
        assert!(
            found.is_empty(),
            "help still says context, namespace, group or application:\n{}",
            found.join("\n")
        );
    }

    /// The current spellings parse.
    #[test]
    fn the_current_spellings_parse() {
        for args in [
            &["meroctl", "attest"][..],
            &["meroctl", "attest", "node"],
            &["meroctl", "attest", "release"],
            &[
                "meroctl", "attest", "release", "--file", "f", "--nonce", "00",
            ],
            &["meroctl", "attest", "app", "c"],
            &["meroctl", "app", "new", "x"],
            &["meroctl", "registry", "list", "--mine"],
            &[
                "meroctl",
                "registry",
                "--registry",
                "http://127.0.0.1:1",
                "search",
                "kv",
                "store",
            ],
            &[
                "meroctl",
                "registry",
                "versions",
                "com.example.kv",
                "--api-key",
                "k",
            ],
            &["meroctl", "registry", "get", "com.example.kv@1.0.0"],
            &["meroctl", "registry", "login"],
            &["meroctl", "package", "install", "com.example.kv"],
            &["meroctl", "package", "list"],
            &["meroctl", "package", "versions", "com.example.kv"],
            &["meroctl", "cloud", "compute", "logs", "a", "r"],
            &["meroctl", "cloud", "compute", "pipeline", "runs", "a"],
            &[
                "meroctl", "local", "compute", "run", "c.wasm", "--method", "m", "--out", "o",
            ],
            &[
                "meroctl", "app", "codegen", "--abi", "a.json", "--lang", "ts",
            ],
            &[
                "meroctl", "app", "abi", "diff", "old.mpk", "new.wasm", "--json",
            ],
            &[
                "meroctl", "instance", "create", "--name", "kv", "--path", "a.wasm",
            ],
            &["meroctl", "call", "--instance", "kv", "get"],
            &["meroctl", "call", "-i", "kv", "get"],
            &["meroctl", "blob", "list", "-i", "kv"],
            &["meroctl", "org", "create", "acme"],
            &["meroctl", "org", "teams", "o"],
            &["meroctl", "org", "create-team", "o", "eng"],
            &["meroctl", "org", "invite", "o", "--team", "t"],
            &["meroctl", "team", "get", "o", "t"],
            &["meroctl", "team", "subteams", "o", "t"],
            &[
                "meroctl",
                "team",
                "upgrade",
                "trigger",
                "o",
                "--package",
                "p",
                "--version",
                "1",
                "--team",
                "t",
            ],
            &[
                "meroctl",
                "instance",
                "create",
                "--name",
                "kv",
                "--package",
                "p",
                "--org",
                "o",
                "--team",
                "t",
            ],
            &["meroctl", "instance", "access", "kv", "--grant", "team:t"],
            &[
                "meroctl",
                "ops",
                "upgrade-org",
                "o",
                "--package",
                "p",
                "--version",
                "1",
            ],
            // Docker is the default; `--docker` from before stays accepted.
            &["meroctl", "local", "up", "--docker"],
            &["meroctl", "local", "up", "--local"],
            &["meroctl", "local", "dev", "app", "--local"],
        ] {
            let parsed = Cli::try_parse_from(args);
            assert!(parsed.is_ok(), "{args:?}: {}", parsed.unwrap_err());
        }
        assert!(
            Cli::try_parse_from(["meroctl", "local", "up", "--docker", "--local"]).is_err(),
            "--docker and --local contradict"
        );
    }

    /// The spellings rc.30 warned about are gone: each is refused, and the
    /// refusal can name what replaced it.
    #[test]
    fn old_spellings_are_refused_and_named() {
        for line in [
            "meroctl dev new x",
            "meroctl attest-release",
            "meroctl attest --context c",
            "meroctl run logs a r",
            "meroctl pipeline runs a",
            "meroctl context create --name kv --path a.wasm",
            "meroctl call --context kv get",
            "meroctl call -c kv get",
            "meroctl blob list -c kv",
            "meroctl namespace create acme",
            "meroctl ns list",
            "meroctl org groups o",
            "meroctl org create-group o eng",
            "meroctl org invite o --group t",
            "meroctl group get o t",
            "meroctl team subgroups o t",
            "meroctl instance create --name kv --package p --namespace o",
            "meroctl ops upgrade-namespace o --package p --version 1",
            "meroctl package build",
            "meroctl package pack --wasm a.wasm",
        ] {
            let args: Vec<String> = line.split(' ').map(str::to_owned).collect();
            assert!(Cli::try_parse_from(&args).is_err(), "{line} still parses");
            assert!(
                !crate::old_spellings(&args).is_empty(),
                "{line} is refused without naming its replacement"
            );
        }
    }

    /// Each old spelling is named with its replacement, and a new one, or a
    /// value that happens to be an old word, is not.
    #[test]
    fn old_spellings_are_named_with_their_replacements() {
        let found = |line: &str| {
            let args: Vec<String> = line.split(' ').map(str::to_owned).collect();
            crate::old_spellings(&args)
        };
        assert_eq!(
            found("meroctl context get kv"),
            [("meroctl context", "meroctl instance")]
        );
        assert_eq!(
            found("meroctl --node group context get kv"),
            [("meroctl context", "meroctl instance")],
            "a global option's value is not the command"
        );
        assert_eq!(found("meroctl call -c kv get"), [("-c", "-i")]);
        assert_eq!(
            found("meroctl call --context=kv get"),
            [("--context", "--instance")]
        );
        assert_eq!(
            found("meroctl namespace create-group o eng"),
            [
                ("meroctl namespace", "meroctl org"),
                ("create-group", "create-team")
            ]
        );
        assert_eq!(
            found("meroctl ns groups o"),
            [("meroctl namespace", "meroctl org"), ("groups", "teams")]
        );
        assert_eq!(
            found("meroctl group subgroups o t"),
            [("meroctl group", "meroctl team"), ("subgroups", "subteams")]
        );
        assert_eq!(
            found("meroctl instance create --name kv --package p --namespace o --group t"),
            [("--namespace", "--org"), ("--group", "--team")]
        );
        assert_eq!(
            found("meroctl ops upgrade-namespace o"),
            [("upgrade-namespace", "upgrade-org")]
        );
        assert_eq!(
            found("meroctl attest --context kv"),
            [("meroctl attest --context", "meroctl attest app")]
        );
        assert_eq!(
            found("meroctl dev new my-app"),
            [("meroctl dev new", "meroctl app new")]
        );
        for current in [
            "meroctl instance get kv",
            "meroctl call -i kv get --view",
            "meroctl org create context",
            "meroctl team get o group",
            "meroctl instance create --name group --package p --org o --team t",
            "meroctl --node local org teams o",
            "meroctl attest app kv",
            "meroctl app new my-app",
            "meroctl package install com.example.kv",
            "meroctl app install com.example.kv",
        ] {
            assert_eq!(found(current), [], "{current}");
        }
    }

    /// `meroctl package` handles installed packages; under `app` the same
    /// commands still parse (scripts written before), but its help does not
    /// list them: `app` makes and ships an app.
    #[test]
    fn package_commands_are_packages_and_hidden_under_app() {
        let mut cli = Cli::command();
        cli.build();
        let package = cli.find_subcommand("package").expect("package");
        let names: Vec<String> = package
            .get_subcommands()
            .map(|c| c.get_name().to_owned())
            .collect();
        assert!(names.iter().any(|n| n == "install"), "{names:?}");
        let app = cli.find_subcommand("app").expect("app");
        for name in names.iter().filter(|n| *n != "help") {
            let under_app = app
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("`app {name}` no longer parses"));
            assert!(
                under_app.is_hide_set(),
                "`app {name}` is listed in app's help"
            );
            assert!(
                !package.find_subcommand(name).expect(name).is_hide_set(),
                "`package {name}` is hidden"
            );
        }
        // And nothing `app` lists is a package command.
        for c in app
            .get_subcommands()
            .filter(|c| !c.is_hide_set() && c.get_name() != "help")
        {
            assert!(
                !names.iter().any(|n| n == c.get_name()),
                "`app {}` is listed under app",
                c.get_name()
            );
        }
    }

    /// `meroctl package origins ...`, and `app origins ...` as before.
    #[test]
    fn package_origins_parse_under_both_names() {
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

    /// The deployment commands and `app release`, as the design writes them.
    #[test]
    fn deployment_and_release_commands_parse() {
        for args in [
            &["meroctl", "deployment", "list"][..],
            &["meroctl", "deployment", "get", "acme-apps"],
            &[
                "meroctl",
                "deployment",
                "create",
                "acme-apps",
                "--owner",
                "account",
            ],
            &[
                "meroctl",
                "deployment",
                "create",
                "acme-apps",
                "--owner",
                "org:o1",
                "--dev-passkey",
                "pk.json",
            ],
            &["meroctl", "deployment", "delete", "acme-apps"],
            &[
                "meroctl",
                "deployment",
                "transfer",
                "acme-apps",
                "--to",
                "org:o2",
            ],
            &["meroctl", "deployment", "accept", "d1", "--to", "org:o2"],
            &["meroctl", "deployment", "cancel-transfer", "acme-apps"],
            &[
                "meroctl",
                "deployment",
                "offer",
                "acme-apps",
                "notes",
                "1.4.0",
            ],
            &[
                "meroctl",
                "deployment",
                "offer",
                "acme-apps",
                "notes",
                "1.4.0",
                "--package",
                "com.acme.notes",
            ],
            &["meroctl", "deployment", "withdraw", "acme-apps", "notes"],
            &["meroctl", "deployment", "secret", "list", "acme-apps"],
            &[
                "meroctl",
                "deployment",
                "secret",
                "set",
                "acme-apps",
                "NEAR_AI_API_KEY",
            ],
            &[
                "meroctl",
                "deployment",
                "secret",
                "clear",
                "acme-apps",
                "NEAR_AI_API_KEY",
            ],
            &["meroctl", "deployment", "orgs", "acme-apps"],
            &[
                "meroctl",
                "deployment",
                "usage",
                "acme-apps",
                "--by",
                "org",
                "--since",
                "2026-11-01",
                "--csv",
            ],
            &["meroctl", "app", "release", "--version", "1.4.0"],
            &[
                "meroctl",
                "app",
                "release",
                "apps/notes",
                "--version",
                "1.4.0",
                "--key",
                "k.json",
                "--publish",
                "--org",
                "acme",
                "--public",
            ],
            &[
                "meroctl",
                "app",
                "release",
                "--version",
                "1.4.0",
                "--wasm",
                "a.wasm",
                "--package",
                "com.acme.notes",
            ],
        ] {
            let parsed = Cli::try_parse_from(args);
            assert!(parsed.is_ok(), "{args:?}: {}", parsed.unwrap_err());
        }
        for refused in [
            // A secret's value is never an argument.
            &[
                "meroctl",
                "deployment",
                "secret",
                "set",
                "d",
                "NAME",
                "sk-1",
            ][..],
            &["meroctl", "deployment", "create", "d", "--owner", "team:x"],
            &["meroctl", "deployment", "usage", "d", "--by", "team"],
            &[
                "meroctl",
                "app",
                "release",
                "--version",
                "1",
                "--app-version",
                "2",
            ],
            &[
                "meroctl",
                "app",
                "release",
                "--version",
                "1",
                "--org",
                "acme",
            ],
        ] {
            assert!(Cli::try_parse_from(refused).is_err(), "{refused:?}");
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
