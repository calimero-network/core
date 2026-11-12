//! `meroctl ops`: the operator's commands, with `--ops-token`. The Ops API
//! has no read path into apps' data: counts, placement, releases, packages,
//! quotas, backups and metrics only. Decisions the deployment gave the
//! operator (`merod --release-approvals/--app-upgrades/--outbound-approvals
//! operator`) are made here; the node refuses the others.

use std::path::PathBuf;

use calimero_client::Client;
use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{json, Value};

use crate::env::{json_arg, read_file, Env};

/// What a registered node is for (`POST /ops/nodes`).
#[derive(Clone, Copy, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeRole {
    /// Holds apps and the cluster key.
    Replica,
    /// Only runs compute: never a member, replica or key holder.
    Job,
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
        OpsCmd::Cluster => client.get("ops/cluster").await?,
        OpsCmd::Metrics => {
            print!("{}", client.get_text("ops/metrics").await?);
            return Ok(Value::Null);
        }
        OpsCmd::Placement => client.get("ops/placement").await?,
        OpsCmd::AddNode {
            id,
            addr,
            zone,
            role,
            pool,
        } => {
            let mut body = json!({ "id": id, "addr": addr, "zone": zone, "role": role });
            if !pool.is_empty() {
                body["pool"] = json!(pool);
            }
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
        OpsCmd::Backup { all } => {
            let path = if all {
                "ops/backup?all=true"
            } else {
                "ops/backup"
            };
            client.post(path, json!({})).await?
        }
        OpsCmd::Restore {
            context,
            backup,
            recovery_key_file: Some(key_file),
            recovery_file,
        } => {
            let _none = backup;
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
            let body = json!({ "recovery": { "file": name, "key": hex::encode(*key) } });
            decide_at(
                env,
                client,
                Some(body),
                &format!("ops/apps/{context}/restore"),
            )
            .await?
        }
        OpsCmd::BackupKey { public_key: None } => client.get("ops/backup-key").await?,
        OpsCmd::BackupKey {
            public_key: Some(key),
        } => {
            decide(
                env,
                "ops/backup-key",
                json!({ "publicKey": key.to_ascii_lowercase() }),
            )
            .await?
        }
        OpsCmd::RecoveryKeygen { out } => {
            let (secret, public) = calimero_client::recovery::new_keypair();
            write_secret(&out, &hex::encode(*secret))?;
            json!({ "publicKey": hex::encode(public), "secretKeyFile": out })
        }
        OpsCmd::Recover { key_file } => {
            let secret = read_secret(&key_file)?;
            let files = recovery_files(env, client).await?;
            let open =
                |name: &str, header: &calimero_client::recovery::Header| -> eyre::Result<Value> {
                    let key = calimero_client::recovery::open_key(header, &secret)?;
                    Ok(json!({ "file": name, "key": hex::encode(*key) }))
                };
            use calimero_client::recovery::Kind;
            let (name, header) = files
                .iter()
                .rfind(|(_, h)| h.kind == Kind::System)
                .ok_or_else(|| eyre::eyre!("no system file on that node's recovery disk"))?;
            let system = open(name, header)?;
            let code = files
                .iter()
                .filter(|(_, h)| h.kind == Kind::Code)
                .map(|(n, h)| open(n, h))
                .collect::<eyre::Result<Vec<_>>>()?;
            client
                .post("ops/recover", json!({ "system": system, "code": code }))
                .await?
        }
        OpsCmd::Restore {
            context,
            backup,
            recovery_key_file: None,
            ..
        } => {
            let mut body = json!({});
            if let Some(name) = backup {
                body["backup"] = json!(name);
            }
            decide_at(
                env,
                client,
                Some(body),
                &format!("ops/apps/{context}/restore"),
            )
            .await?
        }
        OpsCmd::Pools => client.get("ops/pools").await?,
        OpsCmd::SetPools { pools } => {
            let body = json!({ "pools": json_arg(&pools)? });
            decide_at(env, client, Some(body), "ops/pools").await?
        }
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
/// The operator's recovery files on that node, oldest first, each checked
/// here: the node's signature over its header and the attestation of the key
/// that made it, under this client's trust policy.
async fn recovery_files(
    env: &Env,
    client: &Client,
) -> eyre::Result<Vec<(String, calimero_client::recovery::Header)>> {
    let listed = client.get("ops/recovery-files?holder=operator").await?;
    let mut out = Vec::new();
    for f in listed["files"].as_array().into_iter().flatten() {
        let name = f["name"].as_str().unwrap_or_default().to_owned();
        let signature: [u8; 64] = hex::decode(f["signature"].as_str().unwrap_or_default())?
            .try_into()
            .map_err(|_| eyre::eyre!("{name}: the signature is not 64 bytes"))?;
        let header = calimero_client::recovery::verify_header(
            f["header"].as_str().unwrap_or_default().as_bytes(),
            &signature,
        )
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
