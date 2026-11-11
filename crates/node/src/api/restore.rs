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

use axum::extract::{Path, State};
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
    #[serde(default)]
    operator_proof: Option<OperatorProofBody>,
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
