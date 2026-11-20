//! The API's routes for undoing an upgrade from its restore point (C9;
//! `SystemCmd::UndoUpgrade`, `crate::node::restore_points`).

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers read `api.rs`'s items, as every area's module does"
)]
use super::*;

use crate::types::{AppRecord, RestorePoint, UNDO_WINDOW_SECS};

/// How long `POST /apps/{app}/undo` waits for the restored groups to take calls before it
/// answers `restored: false` (the restore goes on; `GET /apps/{app}/undo` follows it).
pub const UNDO_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UndoBody {
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// One app an undo reverts, as the routes answer it: the release it leaves and the one
/// it returns to, when, and what is lost (`lostWrites`: what its group applied since
/// the restore point, `entriesSince`, and its last write, `lastWriteAt`; `any` when it
/// wrote after the restore point was taken, which the restore takes back too).
async fn part_json(node: &Node, a: &AppRecord, p: &RestorePoint) -> Value {
    let status = match &p.file {
        Some(f) => Some(node.restore_point_status(&a.app_id, f).await),
        None => None,
    };
    let (lost, available) = match &status {
        Some(Ok(v)) => {
            let last = v["lastWriteMs"].as_u64();
            (
                json!({
                    "entriesSince": v["entriesSince"],
                    "lastWriteAt": last.map(|ms| ms / 1000),
                    "any": last.is_some_and(|ms| v["takenAt"].as_u64().is_none_or(|t| ms / 1000 >= t)),
                }),
                Value::Null,
            )
        }
        Some(Err(e)) => (Value::Null, json!({ "code": e.code, "message": e.message })),
        None => (
            json!({ "entriesSince": 0, "lastWriteAt": null, "any": false }),
            Value::Null,
        ),
    };
    json!({
        "app": a.app_id,
        "from": p.to_release,
        "to": p.from_release,
        "toBundle": p.from_bundle,
        "upgradedAt": p.at,
        "upgradedBy": p.by,
        "auto": p.by == crate::system::AUTO_UPDATE,
        "undoUntil": p.at.saturating_add(UNDO_WINDOW_SECS),
        "restorePoint": p.file.as_ref().map(|f| json!({
            "node": f.node, "index": f.source.index, "sha256": f.source.sha256,
        })),
        "undoneAt": p.undone_at,
        "lostWrites": lost,
        "unavailable": available,
    })
}

/// What an undo of `app` would revert at `now`, and whether it may: `Ok` with each
/// part, or the refusal `UndoUpgrade` would answer.
async fn plan(
    node: &Node,
    app: &AppRecord,
    now: u64,
) -> Result<Vec<(AppRecord, RestorePoint)>, ApiError> {
    node.sys(|s| {
        let set = s.undo_set(&app.app_id)?;
        set.iter()
            .map(|id| {
                let a = s.apps.get(id).ok_or("no such app")?;
                let p = a
                    .restore_point
                    .as_ref()
                    .ok_or(crate::system::NO_RESTORE_POINT)?;
                s.may_undo(a, p, now)?;
                Ok((a.clone(), p.clone()))
            })
            .collect::<Result<Vec<_>, &'static str>>()
    })
    .map_err(crate::node::rejected)
}

/// `GET /apps/{app}/undo`: the upgrade an owner or admin may undo now, for whoever
/// `GET /apps/{app}` admits: `{ app, available, reason, undoUntil, apps }`, each part
/// as [`part_json`] answers it. `available: false` with the refusal's `reason` when there
/// is nothing to undo, the window passed or the app moved on since.
async fn app_undo_view(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let (app, _p) = super::upgrades::readable_app(&node, &headers, &app_id).await?;
    let now = crate::types::update_now();
    let parts = match plan(&node, &app, now).await {
        Ok(parts) => parts,
        Err(e) => {
            return Ok(Json(json!({
                "app": app.app_id,
                "available": false,
                "reason": { "code": e.code, "message": e.message },
                "apps": [],
            })))
        }
    };
    let mut apps = Vec::new();
    for (a, p) in &parts {
        apps.push(part_json(&node, a, p).await);
    }
    let blocked = apps.iter().any(|a| !a["unavailable"].is_null());
    Ok(Json(json!({
        "app": app.app_id,
        "available": !blocked,
        "reason": apps.iter().map(|a| a["unavailable"].clone()).find(|u| !u.is_null()),
        "undoUntil": parts.first().map(|(_, p)| p.at.saturating_add(UNDO_WINDOW_SECS)),
        "apps": apps,
    })))
}

/// `POST /apps/{app}/undo`: undo the app's last upgrade within a week of it, with an
/// owner's or admin's passkey over `<app> / __undo_upgrade / SystemState::undo_args`:
/// the app (and the members its unit's upgrade moved) go back to the release they ran
/// and to their restore point's state, whatever they wrote since (the answer's
/// `lostWrites`). Each restore is seeded by the node holding its restore point; the
/// answer waits up to [`UNDO_WAIT`] for every group to take calls (`restored`).
async fn app_undo(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<UndoBody>,
) -> ApiResult {
    let (app, _p) = super::upgrades::readable_app(&node, &headers, &app_id).await?;
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    if !node.sys(|s| s.approves_upgrades(&app.org, &actor)) {
        return Err(ApiError::forbidden(crate::system::NOT_AN_UPGRADE_APPROVER));
    }
    node.require_features(&[Feature::RestorePoints]).await?;
    let now = crate::types::update_now();
    let parts = plan(&node, &app, now).await?;
    // Each restore point is on its node and whole, and every node that will hold the
    // restored state may hold the app, before the passkey is asked for.
    let mut apps = BTreeMap::new();
    let mut shown = Vec::new();
    for (a, rp) in &parts {
        let part = part_json(&node, a, rp).await;
        if let Some(why) = part["unavailable"].as_object() {
            return Err(ApiError::new(
                409,
                "RESTORE_POINT_UNAVAILABLE",
                why.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ));
        }
        shown.push(part);
        let replicas = match &rp.file {
            Some(f) => {
                let mut r = vec![f.node];
                r.extend(node.sys(|s| {
                    a.replicas
                        .iter()
                        .copied()
                        .filter(|id| *id != f.node && s.replica_node(id).is_some())
                        .collect::<Vec<_>>()
                }));
                r.truncate(a.replicas.len().max(1));
                for id in &r {
                    let release = node.release_of_node(*id).await;
                    if !node.sys(|s| s.release_may_hold(a, release.as_deref())) {
                        return Err(ApiError::forbidden(&format!(
                            "node {id} runs release {}, which is not approved for this app",
                            release.as_deref().unwrap_or("(not attested yet)")
                        )));
                    }
                }
                r
            }
            None => Vec::new(),
        };
        let _prev = apps.insert(a.app_id.clone(), replicas);
    }
    node.require_current_release("undoing an upgrade").await?;
    let args = node.sys(|s| s.undo_args(apps.keys()));
    let consent = consent(
        &node,
        &p,
        &app.app_id,
        "__undo_upgrade",
        &args,
        body.confirm,
    )
    .await?;
    let out = node
        .sys_write(&SystemCmd::UndoUpgrade {
            app_id: app.app_id.clone(),
            apps: apps.clone(),
            actor,
            at: now,
            consent,
        })
        .await?;
    // Each restored group seeded by its restore point's holder, then waited for.
    for (id, replicas) in &apps {
        let (Some(group), Some(seed)) = (out["groups"][id].as_u64(), replicas.first()) else {
            continue;
        };
        node.seed_undo(id, group, *seed).await?;
    }
    let deadline = tokio::time::Instant::now() + UNDO_WAIT;
    let restored = loop {
        let _caught_up = node.sys_fresh().await;
        let done = node.sys(|s| {
            apps.keys()
                .all(|id| s.apps.get(id).is_none_or(|a| a.restore.is_none()))
        });
        if done || tokio::time::Instant::now() >= deadline {
            break done;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    };
    Ok(Json(json!({
        "app": app.app_id,
        "groups": out["groups"],
        "restored": restored,
        "apps": shown,
    })))
}

pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new().route("/apps/{app}/undo", get(app_undo_view).post(app_undo))
}
