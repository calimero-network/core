//! `meroctl ops`: the operator's commands, with `--ops-token`. The Ops API
//! has no read path into apps' data: counts, placement, releases, packages,
//! quotas, backups and metrics only. Decisions the deployment gave the
//! operator (`merod --release-approvals/--app-upgrades/--outbound-approvals
//! operator`) are made here; the node refuses the others.

use std::collections::BTreeMap;
use std::path::PathBuf;

use calimero_client::types::{
    OpsOrgUpgradeRequest, OpsPoolsSetRequestPoolsValue, OpsQuotasSetRequest,
    OpsRegisterNodeRequestRole, OpsUpgradeRequest,
};
use calimero_client::OperatorSigner;
use clap::{Subcommand, ValueEnum};
use ed25519_dalek::{Signer, SigningKey};
use eyre::WrapErr;
use serde::Serialize;
use serde_json::Value;

use crate::env::{json_arg, read_file, to_json, Env};

/// What a registered node is for (`POST /ops/nodes`).
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum NodeRole {
    /// Holds apps and the cluster key.
    Replica,
    /// Only runs compute: never a member, replica or key holder.
    Job,
}

impl NodeRole {
    fn wire(self) -> OpsRegisterNodeRequestRole {
        match self {
            Self::Replica => OpsRegisterNodeRequestRole::Replica,
            Self::Job => OpsRegisterNodeRequestRole::Job,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum OpsCmd {
    /// Node, group and directory counts.
    Status,
    /// The cluster's name and identity key: what `merod run --job-node --cluster-identity`
    /// takes, so a job node serves only this cluster.
    Cluster,
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
        #[arg(long, value_enum, default_value_t = NodeRole::Replica)]
        role: NodeRole,
        /// The compute pool of a job node (`ops set-pools`); none is the default pool.
        #[arg(long, default_value = "")]
        pool: String,
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
    /// Write encrypted snapshots of the contexts this node leads, and of the
    /// system group when it leads it.
    Backup {
        /// Every group this node holds, leader or not: what a node of a
        /// cluster that lost its majority still has (disaster recovery).
        #[arg(long)]
        all: bool,
    },
    /// Restore a context from a sealed backup in this node's `--backup-dir`
    /// (`{backup-dir}/{context}/{name}`); this node seeds it and becomes its
    /// first replica. Everything the context wrote after the backup is lost.
    Restore {
        context: String,
        /// The backup's file name; the newest one when left out.
        #[arg(long)]
        backup: Option<String>,
    },
    /// The compute pools and the runs each takes at once.
    Pools,
    /// Set the compute pools, JSON `{ name: { runsPerNode, maxMemoryMib } }`.
    SetPools {
        pools: String,
    },
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
    let ops = client.ops();
    let key = operator_key(env)?;
    let sign = |message: &[u8]| {
        key.as_ref()
            .map(|k| k.sign(message).to_bytes().to_vec())
            .unwrap_or_default()
    };
    let operator: Option<&OperatorSigner<'_>> = key.is_some().then_some(&sign);
    Ok(match cmd {
        OpsCmd::Status => to_json(&ops.status().await?)?,
        OpsCmd::Cluster => to_json(&ops.cluster().await?)?,
        OpsCmd::Metrics => {
            print!("{}", ops.metrics().await?);
            return Ok(Value::Null);
        }
        OpsCmd::Placement => to_json(&ops.placement().await?)?,
        OpsCmd::AddNode {
            id,
            addr,
            zone,
            role,
            pool,
        } => {
            let pool = (!pool.is_empty()).then_some(pool.as_str());
            decided(
                ops.register_node(id, &addr, Some(&zone), pool, Some(role.wire()), operator)
                    .await,
            )?
        }
        OpsCmd::RemoveNode { id } => {
            // Node removal runs on the system leader.
            let id = id.to_string();
            match ops.remove_node(&id, operator).await {
                Err(e) => match leader_of(&e) {
                    Some(leader) => {
                        let at = env.at(&leader)?;
                        at.use_api_key(token).await;
                        decided(at.ops().remove_node(&id, operator).await)?
                    }
                    None => decided::<()>(Err(e))?,
                },
                Ok(v) => to_json(&v)?,
            }
        }
        OpsCmd::Move { context, replicas } => {
            match ops.set_replicas(&context, replicas.clone(), operator).await {
                // Replica moves run on the context's leader.
                Err(e) => match leader_of(&e) {
                    Some(leader) => {
                        let at = env.at(&leader)?;
                        at.use_api_key(token).await;
                        decided(at.ops().set_replicas(&context, replicas, operator).await)?
                    }
                    None => decided::<()>(Err(e))?,
                },
                Ok(v) => to_json(&v)?,
            }
        }
        OpsCmd::Backup { all } => to_json(&ops.backup(all.then_some(true)).await?)?,
        OpsCmd::Restore { context, backup } => {
            decided(ops.restore(&context, backup.as_deref(), operator).await)?
        }
        OpsCmd::Pools => to_json(&ops.pools_get().await?)?,
        OpsCmd::SetPools { pools } => {
            let pools: BTreeMap<String, OpsPoolsSetRequestPoolsValue> =
                serde_json::from_value(json_arg(&pools)?)?;
            decided(ops.pools_set(pools, operator).await)?
        }
        OpsCmd::Quotas => to_json(&ops.quotas_get().await?)?,
        OpsCmd::SetQuotas { quotas } => {
            // The body is closed: a key it does not list is refused here,
            // named, rather than dropped and the rest sent.
            let quotas: OpsQuotasSetRequest =
                serde_json::from_value(json_arg(&quotas)?).wrap_err("not a quota change")?;
            decided(ops.quotas_set(quotas, operator).await)?
        }
        OpsCmd::Release { version, withdraw } => {
            decided(ops.release(&version, Some(!withdraw), operator).await)?
        }
        OpsCmd::Outbound {
            package,
            prefixes,
            replace,
        } => decided(
            ops.outbound(
                &package,
                Some(crate::namespace::prefixes(&prefixes)),
                Some(replace),
                operator,
            )
            .await,
        )?,
        OpsCmd::Packages => to_json(&ops.package_list().await?)?,
        OpsCmd::Install {
            coords,
            version,
            path,
        } => match (path, coords) {
            (Some(path), _) => to_json(&ops.package_upload(read_file(&path)?).await?)?,
            (None, Some(coords)) => {
                let (package, version) = crate::app::split_coords(&coords, version)?;
                to_json(&ops.package_install(&package, &version).await?)?
            }
            (None, None) => unreachable!("clap requires one"),
        },
        OpsCmd::Uninstall { package, version } => match version {
            Some(v) => to_json(&ops.package_version_uninstall(&package, &v).await?)?,
            None => to_json(&ops.package_uninstall(&package).await?)?,
        },
        OpsCmd::Upgrade {
            context,
            version,
            package,
            service,
            bundle,
            migrate,
        } => {
            let body = OpsUpgradeRequest {
                bundle,
                package,
                version,
                service,
                migrate,
            };
            decided(ops.upgrade(&context, body, operator).await)?
        }
        OpsCmd::UpgradeNamespace {
            namespace,
            package,
            version,
            group,
            cascade,
            migrate,
        } => {
            let body = OpsOrgUpgradeRequest {
                package,
                version,
                team: group,
                cascade: Some(cascade),
                migrate,
                confirm: None,
            };
            decided(ops.org_upgrade(&namespace, body, operator).await)?
        }
    })
}

/// The operator's key, from `--operator-key-file` (32-byte seed, hex): it
/// signs the decisions the node asks the operator to sign (`428
/// OPERATOR_SIGNATURE_REQUIRED`, naming the bytes).
fn operator_key(env: &Env) -> eyre::Result<Option<SigningKey>> {
    let Some(file) = env.operator_key_file.as_ref() else {
        return Ok(None);
    };
    let seed: [u8; 32] = hex::decode(std::fs::read_to_string(file)?.trim())?
        .try_into()
        .map_err(|_| eyre::eyre!("{}: not a 32-byte hex key", file.display()))?;
    Ok(Some(SigningKey::from_bytes(&seed)))
}

/// An operator decision's answer; asked for the operator's signature without
/// a key to give it, says how to give one.
fn decided<T: Serialize>(out: Result<T, calimero_client::Error>) -> eyre::Result<Value> {
    match out {
        Err(e) if e.code() == Some("OPERATOR_SIGNATURE_REQUIRED") => Err(eyre::eyre!(
            "{e}: pass --operator-key-file (the operator's Ed25519 key)"
        )),
        other => to_json(&other?),
    }
}

/// The leader a `421 NOT_LEADER` names, if `e` is one.
fn leader_of(e: &calimero_client::Error) -> Option<String> {
    if e.code() != Some("NOT_LEADER") {
        return None;
    }
    e.details()?["leaderUrl"].as_str().map(str::to_owned)
}
