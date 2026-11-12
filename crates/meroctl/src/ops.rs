//! `meroctl ops`: the operator's commands, with `--ops-token`. The Ops API
//! has no read path into apps' data: counts, placement, releases, packages,
//! quotas, backups and metrics only. Decisions the deployment gave the
//! operator (`merod --release-approvals/--app-upgrades/--outbound-approvals
//! operator`) are made here; the node refuses the others.

use std::collections::BTreeMap;
use std::path::PathBuf;

use calimero_client::types::{
    OpsOrgUpgradeRequest, OpsPoolsSetRequestPoolsValue, OpsPricesSetRequestTable,
    OpsQuotasSetRequest, OpsRecoverRequestCodeItem, OpsRecoverRequestSystem,
    OpsRegisterNodeRequestRole, OpsRestoreRequestRecovery, OpsUpgradeRequest,
};
use calimero_client::{Client, OperatorSigner};
use clap::{Subcommand, ValueEnum};
use ed25519_dalek::{Signer, SigningKey};
use eyre::WrapErr;
use serde::Serialize;
use serde_json::{json, Value};

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
        #[arg(long, conflicts_with = "recovery_key_file")]
        backup: Option<String>,
        /// Restore from the node's recovery disk instead (`--recover-from`,
        /// else `--recovery-dir`), with the operator's backup key: the file's
        /// signature and attestation are checked here, and its data key is
        /// opened here; the key never leaves this machine but for that file's.
        #[arg(long)]
        recovery_key_file: Option<PathBuf>,
        /// The recovery file's name; the context's newest when left out.
        #[arg(long, requires = "recovery_key_file")]
        recovery_file: Option<String>,
    },
    /// The operator's backup key, or set it (`--public-key`, 64 hex): the
    /// X25519 key every context's recovery-disk backups are sealed to where
    /// the operator upgrades apps.
    BackupKey {
        #[arg(long)]
        public_key: Option<String>,
    },
    /// Make a backup key pair: the private half (hex) into `--out` (mode
    /// 0600, keep it off every node), the public half printed for
    /// `ops backup-key --public-key`.
    RecoveryKeygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// Found the cluster again from its recovery disk, on a founder started
    /// with `merod run --bootstrap --recover-from DIR`: the newest `system`
    /// file and every code file, each checked and opened here with the
    /// operator's backup key. Then restore each context with
    /// `ops restore CONTEXT --recovery-key-file`.
    Recover {
        #[arg(long)]
        key_file: PathBuf,
    },
    /// The compute pools and the runs each takes at once.
    Pools,
    /// Set the compute pools, JSON `{ name: { runsPerNode, maxMemoryMib } }`.
    SetPools {
        pools: String,
    },
    /// The operator's price table, `{}` until one is set.
    Prices,
    /// Set the price table, JSON `{ version, fuelPerVcpuSecond, vcpuSecond, gibSecond,
    /// connectorGib, storageGibMonth, egressGib }`; the version must be the current one plus one.
    SetPrices {
        table: String,
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
        OpsCmd::Restore {
            context,
            recovery_key_file: Some(key_file),
            recovery_file,
            ..
        } => {
            let secret = read_secret(&key_file)?;
            let files = recovery_files(env, client).await?;
            let prefix = format!("app-{context}.");
            let (name, header) = files
                .into_iter()
                .rfind(|(n, _)| match &recovery_file {
                    Some(want) => n == want,
                    None => n.starts_with(&prefix),
                })
                .ok_or_else(|| eyre::eyre!("no recovery file of {context} on that node"))?;
            let key = calimero_client::recovery::open_key(&header, &secret)?;
            let recovery = OpsRestoreRequestRecovery {
                file: name,
                key: hex::encode(*key),
            };
            decided(ops.restore(&context, None, Some(recovery), operator).await)?
        }
        OpsCmd::Restore {
            context, backup, ..
        } => decided(
            ops.restore(&context, backup.as_deref(), None, operator)
                .await,
        )?,
        OpsCmd::BackupKey { public_key: None } => to_json(&ops.backup_key().await?)?,
        OpsCmd::BackupKey {
            public_key: Some(key),
        } => decided(
            ops.set_backup_key(&key.to_ascii_lowercase(), operator)
                .await,
        )?,
        OpsCmd::RecoveryKeygen { out } => {
            let (secret, public) = calimero_client::recovery::new_keypair();
            write_secret(&out, &hex::encode(*secret))?;
            json!({ "publicKey": hex::encode(public), "secretKeyFile": out })
        }
        OpsCmd::Recover { key_file } => {
            use calimero_client::recovery::Kind;
            let secret = read_secret(&key_file)?;
            let files = recovery_files(env, client).await?;
            let open = |name: &str,
                        header: &calimero_client::recovery::Header|
             -> eyre::Result<(String, String)> {
                let key = calimero_client::recovery::open_key(header, &secret)?;
                Ok((name.to_owned(), hex::encode(*key)))
            };
            let (name, header) = files
                .iter()
                .rfind(|(_, h)| h.kind == Kind::System)
                .ok_or_else(|| eyre::eyre!("no system file on that node's recovery disk"))?;
            let (file, key) = open(name, header)?;
            let code = files
                .iter()
                .filter(|(_, h)| h.kind == Kind::Code)
                .map(|(n, h)| open(n, h).map(|(file, key)| OpsRecoverRequestCodeItem { file, key }))
                .collect::<eyre::Result<Vec<_>>>()?;
            to_json(
                &ops.recover(OpsRecoverRequestSystem { file, key }, Some(code))
                    .await?,
            )?
        }
        OpsCmd::Pools => to_json(&ops.pools_get().await?)?,
        OpsCmd::SetPools { pools } => {
            let pools: BTreeMap<String, OpsPoolsSetRequestPoolsValue> =
                serde_json::from_value(json_arg(&pools)?)?;
            decided(ops.pools_set(pools, operator).await)?
        }
        OpsCmd::Prices => to_json(&ops.prices_get().await?)?,
        OpsCmd::SetPrices { table } => {
            let table: OpsPricesSetRequestTable = serde_json::from_value(json_arg(&table)?)?;
            decided(ops.prices_set(table, operator).await)?
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

/// The operator's recovery files on that node, oldest first, each checked
/// here: the node's signature over its header and the attestation of the key
/// that made it, under this client's trust policy.
async fn recovery_files(
    env: &Env,
    client: &Client,
) -> eyre::Result<Vec<(String, calimero_client::recovery::Header)>> {
    let listed = client.ops().recovery_files(Some("operator")).await?;
    let mut out = Vec::new();
    for f in listed.files {
        let name = f.name;
        let signature: [u8; 64] = hex::decode(&f.signature)?
            .try_into()
            .map_err(|_| eyre::eyre!("{name}: the signature is not 64 bytes"))?;
        let header = calimero_client::recovery::verify_header(f.header.as_bytes(), &signature)
            .map_err(|e| eyre::eyre!("{name}: {e}"))?;
        calimero_client::recovery::verify_attestation(&header, &env.trust.policy())
            .await
            .map_err(|e| eyre::eyre!("{name}: not written by an attested node: {e}"))?;
        out.push((name, header));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// A backup key's private half, from the file `ops recovery-keygen` wrote.
fn read_secret(file: &std::path::Path) -> eyre::Result<zeroize::Zeroizing<[u8; 32]>> {
    let text = zeroize::Zeroizing::new(std::fs::read_to_string(file)?);
    let bytes = zeroize::Zeroizing::new(hex::decode(text.trim())?);
    let key: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| eyre::eyre!("{}: not a 32-byte hex key", file.display()))?;
    Ok(zeroize::Zeroizing::new(key))
}

/// Write a secret to a file of its own, readable by its owner alone.
fn write_secret(file: &std::path::Path, text: &str) -> eyre::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    let _o = options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _o = options.mode(0o600);
    }
    let mut f = options
        .open(file)
        .map_err(|e| eyre::eyre!("{}: {e} (it is never overwritten)", file.display()))?;
    f.write_all(text.as_bytes())?;
    Ok(())
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
