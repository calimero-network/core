//! `meroctl ops`: the operator's commands, with `--ops-token`. The Ops API
//! has no read path into apps' data: counts, placement, releases, packages,
//! quotas, backups and metrics only. Decisions the deployment gave the
//! operator (`merod --release-approvals/--app-upgrades/--outbound-approvals
//! operator`) are made here; the node refuses the others.

use std::path::PathBuf;

use calimero_client::Client;
use clap::Subcommand;
use serde_json::{json, Value};

use crate::env::{json_arg, read_file, Env};

#[derive(Debug, Subcommand)]
pub enum OpsCmd {
    /// Node, group and directory counts.
    Status,
    /// Prometheus metrics (text).
    Metrics,
    /// Where every context's replicas are, and each node's zone, release and
    /// context count (ids and counts only), for rebalancing.
    Placement,
    /// Register a node so the system group admits it (it joins as a learner,
    /// and becomes a voter while the system group has fewer than 3).
    AddNode {
        id: u64,
        /// Its URL.
        addr: String,
        #[arg(long, default_value = "")]
        zone: String,
    },
    /// Remove a node from the cluster once no context keeps a replica on it
    /// (run against the system leader; another node names it).
    RemoveNode {
        id: u64,
    },
    /// Move a context to these replica node ids.
    Move {
        context: String,
        #[arg(long, value_delimiter = ',')]
        replicas: Vec<u64>,
    },
    /// Write encrypted snapshots of the contexts this node leads.
    Backup,
    Quotas,
    /// Replace the quotas with this JSON.
    SetQuotas {
        quotas: String,
    },
    /// Approve (or `--withdraw`) a mero-tee release for every app, where the
    /// operator holds release approvals.
    Release {
        version: String,
        #[arg(long)]
        withdraw: bool,
    },
    /// Approve URL prefixes an application's publisher may call out to,
    /// cluster-wide, where the operator holds outbound approvals.
    Outbound {
        #[arg(long)]
        package: String,
        #[arg(long = "prefix")]
        prefixes: Vec<String>,
        #[arg(long)]
        replace: bool,
    },
    /// Installed applications.
    Packages,
    /// Install an application for the operator's upgrades: `PACKAGE@VERSION`
    /// from the registry, or a signed `.mpk` with `--path`.
    Install {
        #[arg(value_name = "PACKAGE[@VERSION]", required_unless_present = "path")]
        coords: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, conflicts_with = "coords")]
        path: Option<PathBuf>,
    },
    /// Uninstall an application (or one version) the operator installed.
    Uninstall {
        package: String,
        #[arg(long)]
        version: Option<String>,
    },
    /// Update a context to a release, where the operator holds app upgrades.
    Upgrade {
        context: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        package: Option<String>,
        #[arg(long)]
        service: Option<String>,
        #[arg(long, conflicts_with_all = ["version", "package"])]
        bundle: Option<String>,
        #[arg(long)]
        migrate: Option<String>,
    },
    /// Update an organization's contexts (or one team's) to a release, where
    /// the operator holds app upgrades. Follow it with `meroctl group upgrade status`.
    UpgradeNamespace {
        namespace: String,
        #[arg(long)]
        package: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        group: Option<String>,
        #[arg(long, requires = "group")]
        cascade: bool,
        #[arg(long)]
        migrate: Option<String>,
    },
}

pub async fn run(cmd: OpsCmd, env: &Env) -> eyre::Result<Value> {
    let token = env
        .ops_token
        .as_deref()
        .ok_or_else(|| eyre::eyre!("ops commands need --ops-token"))?;
    let client = &env.client;
    client.use_api_key(token).await;
    Ok(match cmd {
        OpsCmd::Status => client.get("ops/status").await?,
        OpsCmd::Metrics => {
            print!("{}", client.get_text("ops/metrics").await?);
            return Ok(Value::Null);
        }
        OpsCmd::Placement => client.get("ops/placement").await?,
        OpsCmd::AddNode { id, addr, zone } => {
            let body = json!({ "id": id, "addr": addr, "zone": zone });
            decide_at(env, client, Some(body), "ops/nodes").await?
        }
        OpsCmd::RemoveNode { id } => {
            // Node removal runs on the system leader.
            let path = format!("ops/nodes/{id}");
            match decide_at(env, client, None, &path).await {
                Err(e) => match leader_of(&e) {
                    Some(leader) => {
                        let at = env.at(&leader)?;
                        at.use_api_key(token).await;
                        decide_at(env, &at, None, &path).await?
                    }
                    None => return Err(e),
                },
                Ok(v) => v,
            }
        }
        OpsCmd::Move { context, replicas } => {
            let path = format!("ops/apps/{context}/replicas");
            let body = json!({ "replicas": replicas });
            match decide_at(env, client, Some(body.clone()), &path).await {
                // Replica moves run on the context's leader.
                Err(e) => match leader_of(&e) {
                    Some(leader) => {
                        let at = env.at(&leader)?;
                        at.use_api_key(token).await;
                        decide_at(env, &at, Some(body), &path).await?
                    }
                    None => return Err(e),
                },
                Ok(v) => v,
            }
        }
        OpsCmd::Backup => client.post("ops/backup", json!({})).await?,
        OpsCmd::Quotas => client.get("ops/quotas").await?,
        OpsCmd::SetQuotas { quotas } => {
            decide_at(env, client, Some(json_arg(&quotas)?), "ops/quotas").await?
        }
        OpsCmd::Release { version, withdraw } => {
            decide(
                env,
                "ops/releases",
                json!({ "version": version, "approved": !withdraw }),
            )
            .await?
        }
        OpsCmd::Outbound {
            package,
            prefixes,
            replace,
        } => {
            let prefixes = if prefixes.is_empty() {
                json!("manifest")
            } else {
                json!(prefixes)
            };
            decide(
                env,
                "ops/outbound",
                json!({ "package": package, "prefixes": prefixes, "replace": replace }),
            )
            .await?
        }
        OpsCmd::Packages => client.get("ops/packages").await?,
        OpsCmd::Install {
            coords,
            version,
            path,
        } => match (path, coords) {
            (Some(path), _) => client.ops_upload_package(read_file(&path)?).await?,
            (None, Some(coords)) => {
                let (package, version) = crate::app::split_coords(&coords, version)?;
                client.ops_install_package(&package, &version).await?
            }
            (None, None) => unreachable!("clap requires one"),
        },
        OpsCmd::Uninstall { package, version } => {
            client
                .ops_uninstall_package(&package, version.as_deref())
                .await?
        }
        OpsCmd::Upgrade {
            context,
            version,
            package,
            service,
            bundle,
            migrate,
        } => {
            decide(
                env,
                &format!("ops/apps/{context}/upgrade"),
                json!({
                    "bundle": bundle, "package": package, "version": version,
                    "service": service, "migrate": migrate,
                }),
            )
            .await?
        }
        OpsCmd::UpgradeNamespace {
            namespace,
            package,
            version,
            group,
            cascade,
            migrate,
        } => {
            decide(
                env,
                &format!("ops/orgs/{namespace}/upgrade"),
                json!({
                    "package": package, "version": version, "team": group,
                    "cascade": cascade, "migrate": migrate,
                }),
            )
            .await?
        }
    })
}

/// Send an operator decision: when the node asks for the operator's
/// signature (`428 OPERATOR_SIGNATURE_REQUIRED`, naming the bytes to sign),
/// sign them with `--operator-key-file` and send it again with the proof.
async fn decide(env: &Env, path: &str, body: Value) -> eyre::Result<Value> {
    decide_at(env, &env.client, Some(body), path).await
}

/// [`decide`] through `client` (another node of the cluster, for a decision
/// that runs on a leader): a POST of `body`, or a DELETE without one.
async fn decide_at(
    env: &Env,
    client: &Client,
    body: Option<Value>,
    path: &str,
) -> eyre::Result<Value> {
    use ed25519_dalek::Signer;
    let Some(file) = env.operator_key_file.as_ref() else {
        let out = match body {
            Some(body) => client.post(path, body).await,
            None => client.delete(path).await,
        };
        return match out {
            Err(e) if e.code() == Some("OPERATOR_SIGNATURE_REQUIRED") => Err(eyre::eyre!(
                "{e}: pass --operator-key-file (the operator's Ed25519 key)"
            )),
            other => Ok(other?),
        };
    };
    let seed: [u8; 32] = hex::decode(std::fs::read_to_string(file)?.trim())?
        .try_into()
        .map_err(|_| eyre::eyre!("{}: not a 32-byte hex key", file.display()))?;
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let sign = |message: &[u8]| key.sign(message).to_bytes().to_vec();
    Ok(match body {
        Some(body) => client.post_operator_signed(path, body, &sign).await?,
        None => client.delete_operator_signed(path, &sign).await?,
    })
}

/// The leader a `421 NOT_LEADER` names, if `e` is one.
fn leader_of(e: &eyre::Report) -> Option<String> {
    let e = e.downcast_ref::<calimero_client::Error>()?;
    if e.code() != Some("NOT_LEADER") {
        return None;
    }
    e.details()?["leaderUrl"].as_str().map(str::to_owned)
}
