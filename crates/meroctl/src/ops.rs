//! `meroctl ops`: the operator's commands, with `--ops-token`. The Ops API
//! has no read path into apps' data: counts, placement, releases, packages,
//! quotas, backups and metrics only. Decisions the deployment gave the
//! operator (`merod --release-approvals/--app-upgrades/--outbound-approvals
//! operator`) are made here; the node refuses the others.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::env::{json_arg, read_file, Env};

#[derive(Debug, Subcommand)]
pub enum OpsCmd {
    /// Node, group and directory counts.
    Status,
    /// Prometheus metrics (text).
    Metrics,
    /// Register a node so the system group admits it (it joins as a learner).
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
        OpsCmd::AddNode { id, addr, zone } => {
            client
                .post("ops/nodes", json!({ "id": id, "addr": addr, "zone": zone }))
                .await?
        }
        OpsCmd::RemoveNode { id } => client.delete(&format!("ops/nodes/{id}")).await?,
        OpsCmd::Move { context, replicas } => {
            let path = format!("ops/apps/{context}/replicas");
            let body = json!({ "replicas": replicas });
            match client.post(&path, body.clone()).await {
                // Replica moves run on the context's leader.
                Err(e) if e.code() == Some("NOT_LEADER") => {
                    let leader = e
                        .details()
                        .and_then(|d| d["leaderUrl"].as_str())
                        .ok_or_else(|| eyre::eyre!("{e}"))?
                        .to_owned();
                    let at = env.at(&leader)?;
                    at.use_api_key(token).await;
                    at.post(&path, body).await?
                }
                other => other?,
            }
        }
        OpsCmd::Backup => client.post("ops/backup", json!({})).await?,
        OpsCmd::Quotas => client.get("ops/quotas").await?,
        OpsCmd::SetQuotas { quotas } => client.post("ops/quotas", json_arg(&quotas)?).await?,
        OpsCmd::Release { version, withdraw } => {
            client
                .post(
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
            client
                .post(
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
            client
                .post(
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
            client
                .post(
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
