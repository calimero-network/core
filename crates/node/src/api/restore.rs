//! `POST /ops/apps/{app}/restore`: restore an app from its sealed backup, as
//! the operator's signed decision (`<app> / __restore_app / { backup, sha256 }`).
//!
//! The node that receives it must hold the backup in its `--backup-dir`
//! (`{backup-dir}/{app}/{name}` with its manifest beside it, `crate::backup`)
//! and becomes the restore's seed (`crate::node::restore`). Everything that
//! can be judged before the operator signs is judged first: the backup is
//! found, opens under this cluster's key for this app, its format and release
//! are ones this node reads, and the bundle its state runs is installed.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::{operator, operator_proof, ApiResult, OperatorProofBody};
use crate::backup::BackupError;
use crate::error::ApiError;
use crate::node::restore::backup_code;
use crate::node::{off_workers, Node};
use crate::system::SystemCmd;
use crate::types::RestoreSource;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RestoreBody {
    /// The backup's file name in `{backup-dir}/{app}/`; the newest when absent.
    #[serde(default)]
    backup: Option<String>,
    /// Restore from a recovery-disk file instead (`--recover-from`, else
    /// `--recovery-dir`), opened with its key.
    #[serde(default)]
    recovery: Option<RecoveryFileBody>,
    #[serde(default)]
    operator_proof: Option<OperatorProofBody>,
}

/// A recovery-disk file in the operator's directory and the data key its
/// holder opened from it (hex): what proves the holder sent it.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecoveryFileBody {
    file: String,
    key: String,
}

impl RecoveryFileBody {
    fn key(&self) -> Result<zeroize::Zeroizing<[u8; 32]>, ApiError> {
        let bytes = zeroize::Zeroizing::new(hex::decode(&self.key).unwrap_or_default());
        let key: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            ApiError::bad_request("BAD_RECOVERY_KEY", "a file's key is 32 bytes of hex")
        })?;
        Ok(zeroize::Zeroizing::new(key))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecoverBody {
    /// The newest `system` file of the operator's directory, and its key.
    system: RecoveryFileBody,
    /// The `code` files of the apps' bundles, each with its key.
    #[serde(default)]
    code: Vec<RecoveryFileBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecoverHolderBody {
    /// `org:<id>` or `account:<id>`.
    holder: String,
    /// The holder's newest `holder` file and its key.
    slice: RecoveryFileBody,
    /// Each app's `app` file to bring back, and its key.
    #[serde(default)]
    apps: Vec<RecoveryFileBody>,
    /// The `code` files of those apps' bundles, each with its key.
    #[serde(default)]
    code: Vec<RecoveryFileBody>,
    #[serde(default)]
    operator_proof: Option<OperatorProofBody>,
}

/// Bring an organization's or account's apps back from its recovery files
/// in this node's `--recovery-dir` (or `--recover-from`), on a cluster that
/// does not hold them: the holder's own device sends the data key of each
/// file, opened there with its backup key, over the sealed channel, so no
/// key passes the operator; the operator admits the import by signing
/// `cluster / __recover_holder / { holder, slice, apps }` (`428` names it
/// first). This node becomes each app's seed. Answers each app's restore.
pub(super) async fn recover_holder(
    State(node): State<Arc<Node>>,
    Json(b): Json<RecoverHolderBody>,
) -> ApiResult {
    use crate::recovery_disk::{self as disk, Kind};
    use crate::system::{import_holder_args, BackupHolder, HolderSlice, ImportedApp};
    let bad = |e: eyre::Report| ApiError::bad_request("BAD_RECOVERY_FILE", &format!("{e:#}"));
    if b.apps.len() + b.code.len() > MAX_RECOVERY_CODE_FILES {
        return Err(ApiError::bad_request(
            "TOO_MANY_FILES",
            &format!("at most {MAX_RECOVERY_CODE_FILES} files"),
        ));
    }
    let holder = match b.holder.split_once(':') {
        Some(("org", id)) if !id.is_empty() => BackupHolder::Org(id.to_owned()),
        Some(("account", id)) if !id.is_empty() => BackupHolder::Account(id.to_owned()),
        _ => {
            return Err(ApiError::bad_request(
                "BAD_REQUEST",
                "holder is org:<id> or account:<id>",
            ))
        }
    };
    let Some(dir) = node
        .config
        .recovery_dir
        .clone()
        .or_else(|| node.config.recover_from.clone())
    else {
        return Err(ApiError::new(
            409,
            "NO_BACKUP_DIR",
            "this node has no --recovery-dir or --recover-from to import from",
        ));
    };
    let me = node.config.node_id;
    if node.sys(|s| s.replica_node(&me).is_none()) {
        return Err(ApiError::bad_request(
            "NOT_A_REPLICA_NODE",
            "send the import to a registered node that holds app replicas",
        ));
    }
    let at = disk::holder_dir(&dir, &holder);
    let open = |f: &RecoveryFileBody, kind: Kind| -> Result<_, ApiError> {
        let opened = off_workers(|| disk::read(&disk::path_in(&at, &f.file)?)).map_err(bad)?;
        if opened.header.kind != kind || opened.header.holder != holder {
            return Err(bad(eyre::eyre!(
                "{} is not a {} file of {}",
                f.file,
                kind.name(),
                holder.key()
            )));
        }
        Ok((opened, f.key()?))
    };

    let (slice_file, slice_key) = open(&b.slice, Kind::Holder)?;
    let body = off_workers(|| slice_file.whole(&slice_key)).map_err(bad)?;
    let slice: HolderSlice = serde_json::from_slice(&body)
        .map_err(|e| bad(eyre::eyre!("{} holds no slice: {e}", b.slice.file)))?;
    let public_key = slice_file.header.recipient.clone();

    let mut files = Vec::new();
    for f in &b.apps {
        let (opened, key) = open(f, Kind::App)?;
        let app_id = opened.header.app.clone().unwrap_or_default();
        if !slice.apps.contains_key(&app_id)
            || files.iter().any(|(id, ..): &(String, _, _)| *id == app_id)
        {
            return Err(bad(eyre::eyre!(
                "{} is not one more app of the slice",
                f.file
            )));
        }
        // The key must open it before the operator is asked to sign.
        let _meta = off_workers(|| opened.app(&key, |_| Ok(()))).map_err(bad)?;
        files.push((app_id, opened, key));
    }
    let mut code = Vec::new();
    for f in &b.code {
        let (opened, key) = open(f, Kind::Code)?;
        let hash = opened.header.hash.clone().unwrap_or_default();
        let bytes = off_workers(|| opened.whole(&key)).map_err(bad)?;
        if hex::encode(ring::digest::digest(&ring::digest::SHA256, &bytes)) != hash {
            return Err(bad(eyre::eyre!(
                "{} does not hold the code it names",
                f.file
            )));
        }
        code.push((hash, bytes));
    }

    // This node first, then the other replica nodes, as many as the cluster
    // gives an app.
    let replicas: Vec<u64> = node.sys(|s| {
        let mut ids = vec![me];
        ids.extend(s.replica_nodes().map(|n| n.id).filter(|id| *id != me));
        ids.truncate(node.config.replicas.max(1));
        ids
    });
    let imported: Vec<ImportedApp> = files
        .iter()
        .map(|(app_id, opened, _)| ImportedApp {
            app_id: app_id.clone(),
            replicas: replicas.clone(),
            source: RestoreSource {
                name: opened
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                sha256: opened.header.body_sha256.clone(),
                index: opened.header.index,
            },
        })
        .collect();
    let slice_sha256 = slice_file.header.body_sha256.clone();
    let proof = operator_proof(
        &node,
        "cluster",
        "__recover_holder",
        &import_holder_args(&holder, &slice_sha256, &imported),
        b.operator_proof,
    )?;
    for (hash, bytes) in &code {
        node.put_code(hash, bytes)?;
    }
    let out = node
        .sys_write(&SystemCmd::ImportHolder {
            holder,
            slice: Box::new(slice),
            slice_sha256,
            public_key,
            seed: me,
            apps: imported,
            at: crate::types::now_secs(),
            proof,
        })
        .await?;
    let mut restored = Vec::new();
    for (app_id, opened, key) in files {
        let group = out["apps"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["app"] == app_id.as_str()))
            .and_then(|x| x["group"].as_u64())
            .ok_or_else(|| ApiError::internal("the import recorded no group"))?;
        let backup = json!({
            "name": opened.path.file_name().map(|n| n.to_string_lossy().into_owned()),
            "index": opened.header.index,
            "format": format!("recovery/{}", opened.header.format),
        });
        let index = opened.header.index;
        let done = node
            .seed_with(&app_id, group, index, backup, |g| {
                g.replace_state_rows(|stage| opened.app(&key, stage).map(|_meta| ()))
            })
            .await?;
        restored.push(done);
    }
    Ok(Json(json!({ "apps": restored })))
}

/// The most recovery files one listing answers.
pub const MAX_LISTED_RECOVERY_FILES: usize = 4096;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecoveryFilesQuery {
    /// `operator` (the default), `org:<id>` or `account:<id>`.
    #[serde(default)]
    holder: Option<String>,
}

/// The recovery files of `holder` this node reads (`--recover-from`, else
/// `--recovery-dir`): each one's name, its header as signed and the node's
/// signature over it, so the holder's device checks them before it opens a
/// key. Never a key or a body.
pub(super) async fn ops_recovery_files(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<RecoveryFilesQuery>,
) -> ApiResult {
    use crate::recovery_disk as disk;
    use crate::system::BackupHolder;
    operator(&node, &headers)?;
    let holder = match q.holder.as_deref().unwrap_or("operator") {
        "operator" => BackupHolder::Operator,
        h => match h.split_once(':') {
            Some(("org", id)) if !id.is_empty() => BackupHolder::Org(id.to_owned()),
            Some(("account", id)) if !id.is_empty() => BackupHolder::Account(id.to_owned()),
            _ => {
                return Err(ApiError::bad_request(
                    "BAD_REQUEST",
                    "holder is operator, org:<id> or account:<id>",
                ))
            }
        },
    };
    let Some(dir) = node
        .config
        .recover_from
        .clone()
        .or_else(|| node.config.recovery_dir.clone())
    else {
        return Err(ApiError::new(
            409,
            "NO_BACKUP_DIR",
            "this node has no --recover-from or --recovery-dir",
        ));
    };
    let at = disk::holder_dir(&dir, &holder);
    let files = off_workers(|| {
        let mut out = Vec::new();
        for path in disk::files_of(&at, "")?
            .into_iter()
            .take(MAX_LISTED_RECOVERY_FILES)
        {
            let Ok(opened) = disk::read(&path) else {
                continue;
            };
            out.push(json!({
                "name": path.file_name().map(|n| n.to_string_lossy().into_owned()),
                "header": String::from_utf8_lossy(&opened.header_bytes),
                "signature": hex::encode(opened.signature),
            }));
        }
        Ok::<_, eyre::Report>(out)
    })
    .map_err(|e| ApiError::internal(&format!("{e:#}")))?;
    Ok(Json(json!({ "holder": holder, "files": files })))
}

/// The most code files one recovery takes.
pub const MAX_RECOVERY_CODE_FILES: usize = 4096;

/// Found the cluster again from its recovery disk: on the founder started
/// with `--bootstrap --recover-from DIR`, the operator sends the keys of its
/// `system` file and the code files (it holds the key they open with); the
/// system state is seeded as a founding from backups seeds it. Every app then
/// waits for `POST /ops/apps/{app}/restore { recovery }`.
pub(super) async fn ops_recover(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(b): Json<RecoverBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    if b.code.len() > MAX_RECOVERY_CODE_FILES {
        return Err(ApiError::bad_request(
            "TOO_MANY_FILES",
            &format!("at most {MAX_RECOVERY_CODE_FILES} code files"),
        ));
    }
    let system = b.system.key()?;
    let code = b
        .code
        .iter()
        .map(|c| Ok((c.file.clone(), *c.key()?)))
        .collect::<Result<Vec<_>, ApiError>>()?;
    let out = node
        .recover_system((&b.system.file, &system), &code)
        .await?;
    Ok(Json(out))
}

/// Restore `app_id` from recovery-disk file `r` (see [`ops_restore`]): the
/// file must be the app's, from this cluster, sealed to the key its holder
/// holds now, and open with the key given.
async fn ops_restore_recovery(
    node: Arc<Node>,
    app_id: String,
    r: RecoveryFileBody,
    signed: Option<OperatorProofBody>,
) -> ApiResult {
    use crate::recovery_disk::{self as disk, Kind};
    let bad = |e: eyre::Report| ApiError::bad_request("BAD_RECOVERY_FILE", &format!("{e:#}"));
    let Some(dir) = node
        .config
        .recover_from
        .clone()
        .or_else(|| node.config.recovery_dir.clone())
    else {
        return Err(ApiError::new(
            409,
            "NO_BACKUP_DIR",
            "this node has no --recover-from or --recovery-dir to restore from",
        ));
    };
    let app = node.app(&app_id).await?;
    let me = node.config.node_id;
    if node.sys(|s| s.replica_node(&me).is_none()) {
        return Err(ApiError::bad_request(
            "NOT_A_REPLICA_NODE",
            "send the restore to a registered node that holds app replicas",
        ));
    }
    let holder = node.sys(|s| s.backup_holder(&app));
    let at = disk::holder_dir(&dir, &holder);
    let opened = off_workers(|| disk::read(&disk::path_in(&at, &r.file)?)).map_err(bad)?;
    let h = &opened.header;
    if h.kind != Kind::App || h.app.as_deref() != Some(app_id.as_str()) {
        return Err(bad(eyre::eyre!(
            "{} is not a recovery file of {app_id}",
            r.file
        )));
    }
    if h.cluster != node.config.cluster || h.holder != holder {
        return Err(bad(eyre::eyre!(
            "{} was written for {} of cluster {}, not this app's holder here",
            r.file,
            h.holder.key(),
            h.cluster
        )));
    }
    let key = r.key()?;
    // The key must open it before the operator is asked to sign.
    let meta = off_workers(|| opened.app(&key, |_| Ok(()))).map_err(bad)?;
    let bundle = meta.record["bundle"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let compute_bundle = meta.record["computeBundle"].as_str().map(str::to_owned);
    if node.sys(|s| !s.bundles.contains_key(&bundle)) {
        return Err(ApiError::new(
            409,
            "BUNDLE_GONE",
            crate::system::RESTORE_BUNDLE_GONE,
        ));
    }
    let mut replicas = vec![me];
    replicas.extend(node.sys(|s| {
        app.replicas
            .iter()
            .copied()
            .filter(|id| *id != me && s.replica_node(id).is_some())
            .collect::<Vec<_>>()
    }));
    replicas.truncate(app.replicas.len().max(1));
    for id in &replicas {
        let release = node.release_of_node(*id).await;
        if !node.sys(|s| s.release_may_hold(&app, release.as_deref())) {
            return Err(ApiError::forbidden(&format!(
                "node {id} runs release {}, which this app's owner has not approved",
                release.as_deref().unwrap_or("(not attested yet)")
            )));
        }
    }
    node.require_current_release("restoring an app").await?;
    let source = RestoreSource {
        name: r.file.clone(),
        sha256: h.body_sha256.clone(),
        index: h.index,
    };
    let proof = operator_proof(
        &node,
        &app_id,
        "__restore_app",
        &json!({ "backup": source.name, "sha256": source.sha256 }),
        signed,
    )?;
    let Some(guard) = node.begin_move(&app_id) else {
        return Err(ApiError::new(
            409,
            "MOVE_IN_PROGRESS",
            "this app's replicas are being moved or restored; retry once that answers",
        ));
    };
    let index = h.index;
    let backup = json!({
        "name": r.file,
        "index": index,
        "format": format!("recovery/{}", h.format),
        "merod": h.merod,
        "release": h.release,
    });
    tokio::spawn(async move {
        let _moving = guard;
        let out = node
            .sys_write(&SystemCmd::RestoreApp {
                app_id: app_id.clone(),
                replicas,
                bundle,
                compute_bundle,
                source,
                proof,
            })
            .await?;
        let group = out["group"]
            .as_u64()
            .ok_or_else(|| ApiError::internal("the restore recorded no group"))?;
        node.seed_with(&app_id, group, index, backup, |g| {
            g.replace_state_rows(|stage| opened.app(&key, stage).map(|_meta| ()))
        })
        .await
        .map(Json)
    })
    .await
    .map_err(|e| ApiError::internal(&e.to_string()))?
}

fn refused(e: &BackupError) -> ApiError {
    let status = match e {
        BackupError::NotFound(_) => 404,
        BackupError::Bad(_) => 400,
        BackupError::UnknownFormat { .. } | BackupError::NewerRelease { .. } => 409,
    };
    ApiError::new(status, e.code(), &e.to_string())
}

/// Restore `app` from a backup this node holds; this node seeds it. Answers
/// `{ app, group, rows, backup: { name, index, format, merod, release }, status }`
/// once every replica holds the restored state.
pub(super) async fn ops_restore(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(b): Json<RestoreBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    operator(&node, &headers)?;
    if let Some(r) = b.recovery {
        if b.backup.is_some() {
            return Err(ApiError::bad_request(
                "BAD_REQUEST",
                "name a backup or a recovery file, not both",
            ));
        }
        return ops_restore_recovery(node, app_id, r, b.operator_proof).await;
    }
    let Some(dir) = node.config.backup_dir.clone() else {
        return Err(ApiError::new(
            409,
            "NO_BACKUP_DIR",
            "this node has no --backup-dir to restore from",
        ));
    };
    let app = node.app(&app_id).await?;
    let me = node.config.node_id;
    if node.sys(|s| s.replica_node(&me).is_none()) {
        return Err(ApiError::bad_request(
            "NOT_A_REPLICA_NODE",
            "send the restore to a registered node that holds app replicas",
        ));
    }
    let key = *node.cluster_key();
    let wanted = b.backup.clone();
    let found = off_workers(|| crate::backup::find(&dir, &key, &app_id, wanted.as_deref()))
        .map_err(|e| refused(&e))?;
    found
        .check_release(env!("CARGO_PKG_VERSION"))
        .map_err(|e| refused(&e))?;
    // A backup from before manifests names its group only in its file name,
    // which nothing seals: it is taken only for the group the app is in now.
    if found.manifest.is_none() && found.group != app.group {
        return Err(ApiError::bad_request(
            "BAD_BACKUP",
            "this backup has no manifest and is not of the app's current group",
        ));
    }
    let (bundle, compute_bundle) =
        off_workers(|| backup_code(&key, &found)).map_err(|e| refused(&e))?;
    if node.sys(|s| !s.bundles.contains_key(&bundle)) {
        return Err(ApiError::new(
            409,
            "BUNDLE_GONE",
            crate::system::RESTORE_BUNDLE_GONE,
        ));
    }
    // The seed first, then the app's other replicas that are still registered,
    // as many as it had.
    let mut replicas = vec![me];
    replicas.extend(node.sys(|s| {
        app.replicas
            .iter()
            .copied()
            .filter(|id| *id != me && s.replica_node(id).is_some())
            .collect::<Vec<_>>()
    }));
    replicas.truncate(app.replicas.len().max(1));
    // The restored state reaches only nodes on a release its owner approved,
    // as a move's does.
    for id in &replicas {
        let release = node.release_of_node(*id).await;
        if !node.sys(|s| s.release_may_hold(&app, release.as_deref())) {
            return Err(ApiError::forbidden(&format!(
                "node {id} runs release {}, which this app's owner has not approved",
                release.as_deref().unwrap_or("(not attested yet)")
            )));
        }
    }
    // Nodes of an earlier release would not know the restore's entries.
    node.require_current_release("restoring an app").await?;
    let source = RestoreSource {
        name: found.name.clone(),
        sha256: found.sha256.clone(),
        index: found.index,
    };
    let proof = operator_proof(
        &node,
        &app_id,
        "__restore_app",
        &json!({ "backup": source.name, "sha256": source.sha256 }),
        b.operator_proof,
    )?;
    let Some(guard) = node.begin_move(&app_id) else {
        return Err(ApiError::new(
            409,
            "MOVE_IN_PROGRESS",
            "this app's replicas are being moved or restored; retry once that answers",
        ));
    };
    // The restore runs to the end in a task of its own, which holds the
    // guard: a caller that disconnects does not leave it half done.
    tokio::spawn(async move {
        let _moving = guard;
        let out = node
            .sys_write(&SystemCmd::RestoreApp {
                app_id: app_id.clone(),
                replicas,
                bundle,
                compute_bundle,
                source,
                proof,
            })
            .await?;
        let group = out["group"]
            .as_u64()
            .ok_or_else(|| ApiError::internal("the restore recorded no group"))?;
        node.seed_restore(&app_id, group, &found).await.map(Json)
    })
    .await
    .map_err(|e| ApiError::internal(&e.to_string()))?
}
