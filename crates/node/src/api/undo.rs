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
/// it returns to, when, and what is lost ([`lost_writes`]).
async fn part_json(node: &Node, a: &AppRecord, p: &RestorePoint) -> Value {
    let status = match &p.file {
        Some(f) => Some(node.restore_point_status(&a.app_id, f).await),
        None => None,
    };
    let (lost, available) = match (&status, &p.file) {
        (Some(Ok(v)), Some(f)) => {
            // What the app wrote since is its replicas' to say, a majority of them, whether
            // or not the restore point's node still hosts it: that node may be behind.
            let progress = node.group_progress(a).await;
            (lost_writes(v, f.source.index, progress), Value::Null)
        }
        (Some(Err(e)), _) => (Value::Null, json!({ "code": e.code, "message": e.message })),
        _ => (
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

/// What an undo loses (`lostWrites`): the app group's entries since the restore point
/// (`entriesSince`, taken at `index`), its last write (`lastWriteAt`, Unix seconds), and
/// `any` when it wrote after the restore point was taken (`holder`'s `takenAt`), which the
/// restore takes back too. Read from `progress`, the furthest of a majority of the app's
/// current replicas ([`Node::group_progress`]; `None` when no majority answered), never
/// from the restore point's node alone, which may be behind its group even while it hosts
/// the app. Only an applied index at or past `index` counts entries or says no write is
/// lost: a replica behind the restore point has not seen what came after it. What nobody
/// can say is `null`, `any` included: an undo never says it loses no writes it cannot see.
fn lost_writes(
    holder: &Value,
    index: u64,
    progress: Option<crate::node::ReplicaProgress>,
) -> Value {
    let unknown = json!({ "entriesSince": null, "lastWriteAt": null, "any": null });
    let taken = holder["takenAt"].as_u64();
    let wrote_since = |ms: u64| taken.is_none_or(|t| ms / 1000 >= t);
    let Some(r) = progress else {
        return unknown;
    };
    let caught_up = r.applied.is_some_and(|i| i >= index);
    let wrote = r.last_write_ms.is_some_and(wrote_since);
    if !caught_up && !wrote {
        return unknown;
    }
    // Past `index`, or a write after the restore point seen even so: a write since is a
    // write lost, whoever saw it, and none is said only by a replica past `index`.
    json!({
        "entriesSince": r.applied.filter(|_| caught_up).map(|i| i - index),
        "lastWriteAt": r.last_write_ms.map(|ms| ms / 1000),
        "any": wrote,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::ReplicaProgress;

    #[test]
    fn lost_writes_are_unknown_when_no_replica_answers_never_none() {
        let holder = json!({
            "hosted": false, "entriesSince": null, "lastWriteMs": null, "takenAt": 100,
        });
        // The holder alone would read as no writes: never answered so.
        let unknown = lost_writes(&holder, 3, None);
        assert_eq!(
            unknown,
            json!({ "entriesSince": null, "lastWriteAt": null, "any": null })
        );
        // A current replica says what it wrote since.
        let read = lost_writes(
            &holder,
            3,
            Some(ReplicaProgress {
                applied: Some(7),
                last_write_ms: Some(150_000),
            }),
        );
        assert_eq!(
            read,
            json!({ "entriesSince": 4, "lastWriteAt": 150, "any": true })
        );
        // A replica on a release before has no index: it cannot show it is past the
        // restore point, so its old last write says nothing either.
        let older = lost_writes(
            &holder,
            3,
            Some(ReplicaProgress {
                applied: None,
                last_write_ms: Some(50_000),
            }),
        );
        assert_eq!(
            older,
            json!({ "entriesSince": null, "lastWriteAt": null, "any": null })
        );
        // A replica that has not applied up to the restore point (index 3) is behind:
        // what it says of writes since is unknown, never `0` and `false`.
        let lagging = lost_writes(
            &holder,
            3,
            Some(ReplicaProgress {
                applied: Some(2),
                last_write_ms: Some(50_000),
            }),
        );
        assert_eq!(
            lagging,
            json!({ "entriesSince": null, "lastWriteAt": null, "any": null }),
            "a lagging replica never says nothing is lost"
        );
        // Behind, yet it saw a write after the restore point: that write is lost.
        let seen = lost_writes(
            &holder,
            3,
            Some(ReplicaProgress {
                applied: Some(2),
                last_write_ms: Some(150_000),
            }),
        );
        assert_eq!(
            seen,
            json!({ "entriesSince": null, "lastWriteAt": 150, "any": true })
        );
        // Past it and no write since: nothing is lost.
        let none = lost_writes(
            &holder,
            3,
            Some(ReplicaProgress {
                applied: Some(3),
                last_write_ms: Some(50_000),
            }),
        );
        assert_eq!(
            none,
            json!({ "entriesSince": 0, "lastWriteAt": 50, "any": false })
        );
        // The holder hosting the app but behind its group (Bugbot on #5210, "lagging
        // replica undercounts"): its own count is never read; the majority's furthest is.
        let hosted = json!({
            "hosted": true, "entriesSince": 0, "lastWriteMs": null, "takenAt": 100,
        });
        assert_eq!(
            lost_writes(
                &hosted,
                3,
                Some(ReplicaProgress {
                    applied: Some(5),
                    last_write_ms: Some(120_000),
                }),
            ),
            json!({ "entriesSince": 2, "lastWriteAt": 120, "any": true })
        );
    }
}
