//! Restoring an app from its sealed backup (`POST /ops/apps/{app}/restore`,
//! `crate::backup`).
//!
//! The log records the operator's decision (`SystemCmd::RestoreApp`): the app
//! moves to a new group, on the bundle the backup's state runs, with this node,
//! which holds the backup, as its seed and first replica. The seed then
//!
//! 1. forms the new group alone (`ensure_app_group` names only the seed as a
//!    voter while the restore is under way) and leads it;
//! 2. replaces the group's empty state with the backup's rows, sealed again
//!    for the new group (`calimero_log::Group::replace_state`);
//! 3. appends `componentRestored`, so the new group's run ids continue past
//!    every term the restored state carries (`ComponentHead::id_term_base`);
//! 4. builds a snapshot through that entry and purges its log through it, so
//!    a replica added from now on can only catch up from that snapshot;
//! 5. hands the others the bundle and blobs, adds them as learners and makes
//!    them voters, as a move does;
//! 6. records `FinishRestore`, and the app takes calls again.
//!
//! The old group is stopped on every replica once the directory names the new
//! one (`Node::reconcile`), so nothing written to the old group after the
//! backup survives: the restore is a rollback of that app to the backup, which
//! is why it is the operator's signed decision. A seed that stops half way
//! leaves the restore under way (calls answer `409 APP_RESTORING`); sending
//! the restore again starts over in a newer group.
//!
//! A cluster is founded again from its backups the same way, one level up
//! ([`Node::refound`], `merod run --bootstrap --restore-from DIR`): the
//! founder takes the cluster's own K_c from the copy kept for its release
//! (`backup::cluster_key_from`), forms the system group alone, seeds it with
//! the newest system backup, made the state of the new fleet
//! (`system::refound`), snapshots and purges its log, and the other nodes join
//! as registered learners, catching up from that snapshot. Every app then
//! waits for its own backup (`PendingRestore::AWAITING_BACKUP`).

use std::collections::BTreeSet;
use std::time::Duration;

use calimero_log::{Group, GroupId, NodeId};
use serde_json::{json, Value};
use tracing::info;

use super::{Node, SYSTEM_GROUP};
use crate::app::AppMachine;
use crate::backup::Found;
use crate::component::ComponentCmd;
use crate::error::ApiError;
use crate::system::{normalize_addr, SystemCmd, SystemState};
use crate::types::{AppRecord, NodeRecord, NodeRole};

/// How long the seed waits for its new group to elect it, for a snapshot and
/// for the purge.
pub const SEED_WAIT: Duration = Duration::from_secs(30);

/// How long the seed waits for each other replica to catch up as a learner.
pub const JOIN_WAIT: Duration = Duration::from_secs(120);

/// What a call or view of an app under restore answers.
pub const APP_RESTORING: &str = "APP_RESTORING";

impl Node {
    /// `409 APP_RESTORING` while `app` is being restored: its new group holds
    /// the backup on the seed alone, or not yet. Judged on fresh system state,
    /// so a node that has not applied the restore's end yet does not refuse.
    pub(crate) async fn refuse_while_restoring(&self, app: &AppRecord) -> Result<(), ApiError> {
        if app.restore.is_none() {
            return Ok(());
        }
        let _caught_up = self.sys_fresh().await;
        if self.sys(|s| s.apps.get(&app.app_id).is_none_or(|a| a.restore.is_none())) {
            return Ok(());
        }
        Err(ApiError::new(
            409,
            APP_RESTORING,
            "this app is being restored from a backup; retry once the restore answers",
        ))
    }
}

/// The bundle and compute component the backed-up state runs, read from its
/// head; reading it opens every frame and row of the backup, so a backup that
/// does not open under this cluster's key, or is cut short, is refused here.
pub(crate) fn backup_code(
    cluster_key: &[u8; 32],
    found: &Found,
) -> Result<(String, Option<String>), crate::backup::BackupError> {
    let bad = |e: &dyn std::fmt::Display| crate::backup::BackupError::Bad(e.to_string());
    let mut head = None;
    calimero_log::read_snapshot_file(
        cluster_key,
        found.group,
        &found.snapshot_id,
        &found.path,
        |key, value| {
            if key == crate::app::rows::HEAD {
                head = Some(value.to_vec());
            }
            Ok(())
        },
    )
    .map_err(|e| bad(&format!("the backup does not open on this cluster: {e:#}")))?;
    let head = head.ok_or_else(|| bad(&"the backup holds no app state"))?;
    let (bundle, compute) = crate::app::backup_code(&head).map_err(|e| bad(&e))?;
    let bundle = bundle.ok_or_else(|| bad(&"the backed-up app never ran a bundle"))?;
    Ok((bundle, compute))
}

/// How long the founder waits for a node of the cluster backed up to answer
/// at its old address, before founding the cluster again.
pub const OLD_NODE_PROBE: Duration = Duration::from_secs(3);

/// The system state a system backup holds: its in-memory machine's checkpoint.
fn system_state_of(cluster_key: &[u8; 32], found: &Found) -> eyre::Result<SystemState> {
    if found.group != SYSTEM_GROUP {
        eyre::bail!("{} is not a backup of the system group", found.name);
    }
    let mut data = None;
    calimero_log::read_snapshot_file(
        cluster_key,
        found.group,
        &found.snapshot_id,
        &found.path,
        |key, value| {
            if key == calimero_log::machine::CHECKPOINT_KEY {
                data = Some(value.to_vec());
            }
            Ok(())
        },
    )?;
    let data = data.ok_or_else(|| eyre::eyre!("{} holds no system state", found.name))?;
    Ok(serde_json::from_slice(&data)?)
}

impl Node {
    /// On the founder of a cluster founded again (`--bootstrap
    /// --restore-from DIR`), while its system state is empty: seed the system
    /// group, which it is the only member of, with the newest system backup in
    /// `DIR`, made the state of this fleet (`system::refound`), and make that
    /// the only way the other nodes catch up.
    ///
    /// Refused, leaving the state empty, when the backup is not this
    /// release's to read, when it names another governance or operator key
    /// than this node's configuration (the node attests its governance), or
    /// while a replica node of the cluster backed up still answers at an
    /// address the new fleet does not reuse: two live clusters must never
    /// share one K_c.
    pub(crate) async fn refound(&self) -> eyre::Result<()> {
        let Some(dir) = self.config.restore_from.clone() else {
            return Ok(());
        };
        let key = self.cluster_key;
        let found =
            super::off_workers(|| crate::backup::find(&dir, &key, crate::backup::SYSTEM, None))?;
        found.check_release(env!("CARGO_PKG_VERSION"))?;
        let mut state = super::off_workers(|| system_state_of(&key, &found))?;
        if state.governance.clone().unwrap_or_default() != self.config.governance {
            eyre::bail!(
                "the backed-up cluster was deployed with governance {:?}; start the founder with \
                 the same flags",
                state.governance.unwrap_or_default()
            );
        }
        let operator = self.operator_key().map(|k| k.key);
        if state.operator_key.as_ref().map(|k| &k.key) != operator.as_ref() {
            eyre::bail!(
                "the backed-up cluster's operator key is {:?}; start the founder with that \
                 --operator-key",
                state.operator_key.as_ref().map(|k| &k.key)
            );
        }
        self.refuse_while_old_nodes_answer(&state).await?;
        for (hash, bytes) in crate::backup::code(&dir, &key)? {
            self.engine.put_blob(&hash, &bytes)?;
        }
        let founders: Vec<NodeRecord> = self
            .config
            .peers
            .iter()
            .map(|(id, addr)| NodeRecord {
                id: *id,
                addr: addr.clone(),
                zone: String::new(),
                role: NodeRole::Replica,
                pool: String::new(),
            })
            .collect();
        crate::system::refound(&mut state, &founders, self.config.replicas);
        let apps = state.apps.len();
        let accounts = state.accounts.len();
        self.system
            .replace_machine_data(&serde_json::to_vec(&state)?)?;
        let index = self.system.applied();
        self.system.snapshot_through(index, SEED_WAIT).await?;
        self.system.purge_log_through(index, SEED_WAIT).await?;
        info!(
            backup = %found.name,
            index = found.index,
            apps,
            accounts,
            "founded the cluster again from its system backup; restore each app from its own"
        );
        Ok(())
    }

    /// Refuse while a replica node of the cluster backed up, at an address the
    /// new fleet does not reuse, answers as a node of this cluster.
    async fn refuse_while_old_nodes_answer(&self, state: &SystemState) -> eyre::Result<()> {
        let reused: BTreeSet<String> = self
            .config
            .peers
            .values()
            .map(|a| normalize_addr(a))
            .collect();
        for node in state.replica_nodes() {
            if reused.contains(&normalize_addr(&node.addr)) {
                continue;
            }
            let url = format!(
                "{}/attestation?nonce={}",
                node.addr.trim_end_matches('/'),
                hex::encode([0_u8; 32])
            );
            let answer = tokio::time::timeout(OLD_NODE_PROBE, async {
                self.http.get(&url).send().await?.json::<Value>().await
            })
            .await;
            if let Ok(Ok(doc)) = answer {
                if doc["cluster"].as_str() == Some(self.config.cluster.as_str()) {
                    eyre::bail!(
                        "node {} of the cluster backed up still answers at {}: stop every node \
                         of that cluster before founding it again",
                        node.id,
                        node.addr
                    );
                }
            }
        }
        Ok(())
    }

    /// On the seed, once `RestoreApp` put `app_id` in `group`: seed the group
    /// from `found` and grow it to the app's replicas (see the module docs).
    pub(crate) async fn seed_restore(
        &self,
        app_id: &str,
        group: GroupId,
        found: &Found,
    ) -> Result<Value, ApiError> {
        let _caught_up = self.sys_fresh().await;
        let app = self
            .sys_until(|s| s.apps.get(app_id).filter(|a| a.group == group).cloned())
            .await
            .ok_or_else(|| ApiError::unavailable("the restore is not in the directory yet"))?;
        let g = self
            .ensure_app_group(&app)
            .await
            .map_err(|e| ApiError::internal(&format!("cannot start the new group: {e:#}")))?;
        let deadline = tokio::time::Instant::now() + SEED_WAIT;
        while !g.status().is_leader {
            if tokio::time::Instant::now() >= deadline {
                return Err(ApiError::unavailable(
                    "the new group did not elect its seed",
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let rows =
            super::off_workers(|| g.replace_state(found.group, &found.snapshot_id, &found.path))
                .map_err(|e| ApiError::internal(&format!("cannot seed the new group: {e:#}")))?;
        let marker = serde_json::to_vec(&ComponentCmd::Restored {
            from_index: found.index,
        })
        .map_err(|e| ApiError::internal(&e.to_string()))?;
        let (index, _out) = g
            .write(marker)
            .await
            .map_err(|e| ApiError::unavailable(&e.to_string()))?;
        let unavailable = |e: eyre::Report| ApiError::unavailable(&format!("{e:#}"));
        g.snapshot_through(index, SEED_WAIT)
            .await
            .map_err(unavailable)?;
        g.purge_log_through(index, SEED_WAIT)
            .await
            .map_err(unavailable)?;
        self.grow_restored(&app, &g).await?;
        let _ok = self
            .sys_write(&SystemCmd::FinishRestore {
                app_id: app_id.to_owned(),
                group,
            })
            .await?;
        info!(app = %app_id, group, index, rows, "restored an app from its backup");
        Ok(json!({
            "app": app_id,
            "group": group,
            "rows": rows,
            "backup": {
                "name": found.name,
                "index": found.index,
                "format": found.format(),
                "merod": found.manifest.as_ref().map(|m| m.merod.clone()),
                "release": found.manifest.as_ref().map(|m| m.release.clone()),
            },
            "status": g.status(),
        }))
    }

    /// Add the restored app's other replicas to its new group, from the
    /// seed's snapshot, and make every replica a voter.
    async fn grow_restored(&self, app: &AppRecord, g: &Group<AppMachine>) -> Result<(), ApiError> {
        let others: Vec<(NodeId, String)> = self.sys(|s| {
            app.replicas
                .iter()
                .skip(1)
                .filter_map(|id| s.nodes.get(id).map(|n| (*id, n.addr.clone())))
                .collect()
        });
        let _held = self
            .hand_over_bytes(app, g, &others, &std::collections::HashSet::new())
            .await?;
        for (id, addr) in others {
            match tokio::time::timeout(JOIN_WAIT, g.add_learner(id, addr)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    return Err(ApiError::unavailable(&format!(
                        "replica {id} could not join the restored group: {e}"
                    )))
                }
                Err(_) => {
                    return Err(ApiError::unavailable(&format!(
                        "replica {id} did not catch up with the restored group within {JOIN_WAIT:?}"
                    )))
                }
            }
        }
        g.change_voters(app.replicas.iter().copied().collect())
            .await
            .map_err(|e| ApiError::unavailable(&e.to_string()))
    }
}
