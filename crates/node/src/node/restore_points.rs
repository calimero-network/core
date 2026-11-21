//! Restore points: the app group's state just before an upgrade, kept for an undo
//! (`SystemCmd::UndoUpgrade`; docs: protocol upgrades "Restore points and undo").
//!
//! A restore point is the app group's sealed snapshot, brought up to what the replica
//! that takes it has applied, copied with its manifest into that replica's
//! `<data>/restore-points/<app>/` exactly as a backup is written (`crate::backup::write`),
//! so the restore machinery (`Node::seed_restore`) seeds a new group from it. Every
//! upgrade that runs on an app's group takes one first (`api::upgrades::run_upgrade`),
//! on this node when it hosts the app, else on the first replica that answers
//! `internal/restore-point`; `UpgradeApp { restore_point }` names it in the log and the
//! app keeps it as its one undo (`AppRecord::restore_point`). A node keeps, per app, the
//! file the app's record names and any taken in the last [`RESTORE_POINT_GRACE`] (an
//! upgrade whose record is not logged yet); everything else goes in the sweep, as does
//! a file past the undo window, so storage stays bounded at about one copy of each
//! app's state. That copy counts as the app's storage, billed with the rest of its unit.
//!
//! An undo is seeded by the node that holds the file (`seed_undo`); if that node is
//! gone, so is the undo (`409 RESTORE_POINT_UNAVAILABLE`, asked before any passkey).

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tracing::{info, warn};

use super::{off_workers, Node};
use crate::error::ApiError;
use crate::types::{AppRecord, RestorePointFile, RestoreSource, UNDO_WINDOW_SECS};

/// Where restore points go, under the node's data directory.
pub const RESTORE_POINTS_DIR: &str = "restore-points";

/// How long a restore point no record names yet is kept: the upgrade it was taken for
/// may not be logged yet.
pub const RESTORE_POINT_GRACE: Duration = Duration::from_secs(3600);

/// The most restore point files a node keeps of one app: the one its record names and
/// the newest taken since. Taking another drops the oldest beyond it, so a peer that asks
/// for one again and again fills no disk.
pub const MAX_RESTORE_POINTS_PER_APP: usize = 3;

/// How often a node sweeps its restore points.
pub const RESTORE_POINT_SWEEP_EVERY: Duration = Duration::from_secs(600);

/// `409 RESTORE_POINT_UNAVAILABLE`: the node that holds the restore point does not
/// answer, or no longer has it.
fn unavailable(why: &str) -> ApiError {
    ApiError::new(409, "RESTORE_POINT_UNAVAILABLE", why)
}

impl Node {
    /// This node's restore point directory.
    fn restore_points_dir(&self) -> PathBuf {
        self.config.data_dir.join(RESTORE_POINTS_DIR)
    }

    /// Take a restore point of `app_id`, which this node hosts: its group's snapshot,
    /// brought up to what this node applied, copied beside its manifest.
    pub(crate) async fn take_restore_point_here(
        &self,
        app_id: &str,
    ) -> Result<RestorePointFile, ApiError> {
        #[cfg(feature = "testing")]
        if crate::testing::restore_points_fail() {
            return Err(ApiError::unavailable(
                "the restore point was not taken (testing::fail_restore_points)",
            ));
        }
        let g = self
            .hosted(app_id)
            .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
        let internal = |e: &dyn std::fmt::Display| {
            ApiError::internal(&format!("the restore point was not taken: {e}"))
        };
        let (index, snapshot_id, file) =
            self.fresh_snapshot(&g)
                .await
                .map_err(|e| internal(&e))?
                .ok_or_else(|| ApiError::unavailable("the app's group has no snapshot yet"))?;
        let manifest = crate::backup::Manifest {
            format: crate::backup::FORMAT,
            merod: env!("CARGO_PKG_VERSION").to_owned(),
            release: self.own_release(),
            app: app_id.to_owned(),
            group: g.id(),
            index,
            snapshot_id: snapshot_id.clone(),
            sha256: String::new(),
            written_at: crate::types::now_secs(),
            node: self.config.node_id,
        };
        let dir = self.restore_points_dir();
        let key = *self.cluster_key();
        let name = crate::backup::file_name(index, g.id(), &snapshot_id);
        let found = off_workers(|| {
            std::fs::create_dir_all(&dir)?;
            let _new = crate::backup::write(&dir, &key, &file, &manifest)?;
            let found = crate::backup::find(&dir, &key, app_id, Some(&name))
                .map_err(|e| eyre::eyre!("{e}"))?;
            self.keep_newest_restore_points(app_id, &name);
            Ok::<_, eyre::Report>(found)
        })
        .map_err(|e| internal(&format!("{e:#}")))?;
        info!(app = %app_id, index, "took a restore point");
        Ok(RestorePointFile {
            node: self.config.node_id,
            group: g.id(),
            source: RestoreSource {
                name: found.name,
                sha256: found.sha256,
                index: found.index,
            },
        })
    }

    /// Drop `app_id`'s restore point files here beyond [`MAX_RESTORE_POINTS_PER_APP`],
    /// oldest first, never `taken` nor the one its record names.
    fn keep_newest_restore_points(&self, app_id: &str, taken: &str) {
        let named = self.sys(|s| {
            s.apps
                .get(app_id)
                .and_then(|a| a.restore_point.as_ref())
                .and_then(|p| p.file.as_ref())
                .map(|f| f.source.name.clone())
        });
        let app_dir = self.restore_points_dir().join(app_id);
        let Ok(files) = std::fs::read_dir(&app_dir) else {
            return;
        };
        let mut snaps: Vec<(std::time::SystemTime, String)> = files
            .flatten()
            .filter_map(|f| {
                let name = f.file_name().to_string_lossy().into_owned();
                let snap =
                    !name.starts_with('.') && !name.ends_with(crate::backup::MANIFEST_SUFFIX);
                let at = f.metadata().and_then(|m| m.modified()).ok()?;
                snap.then_some((at, name))
            })
            .collect();
        snaps.sort();
        let excess = snaps.len().saturating_sub(MAX_RESTORE_POINTS_PER_APP);
        for (_, name) in snaps.into_iter().take(excess) {
            if name == taken || named.as_deref() == Some(name.as_str()) {
                continue;
            }
            let _gone = std::fs::remove_file(app_dir.join(&name));
            let _gone = std::fs::remove_file(
                app_dir.join(format!("{name}{}", crate::backup::MANIFEST_SUFFIX)),
            );
        }
    }

    /// Take a restore point of `app` before an upgrade: here when this node hosts it,
    /// else on the first of its replicas that answers.
    pub(crate) async fn take_restore_point(
        &self,
        app: &AppRecord,
    ) -> Result<RestorePointFile, ApiError> {
        if self.hosted(&app.app_id).is_some() {
            return self.take_restore_point_here(&app.app_id).await;
        }
        let body = json!({ "app": app.app_id, "op": "take" });
        let mut last = ApiError::unavailable("no replica took a restore point");
        for id in &app.replicas {
            let Some(addr) = self.sys(|s| s.replica_node(id).map(|n| n.addr.clone())) else {
                continue;
            };
            match self.peer_post(&addr, "internal/restore-point", &body).await {
                Ok(v) => {
                    return serde_json::from_value(v)
                        .map_err(|e| ApiError::internal(&e.to_string()))
                }
                Err(e) => last = ApiError::unavailable(&format!("{e:#}")),
            }
        }
        Err(last)
    }

    /// On the node that holds `file`: whether it is there and whole, with what
    /// `app_id`'s group applied since (`entriesSince`), its last write (`lastWriteMs`)
    /// and when the restore point was taken (`takenAt`, Unix seconds, this node's clock
    /// as the write's is its leader's), for the undo's answer. `hosted: false` (and
    /// both counts `null`) when this node no longer hosts the app (a replica move took
    /// it off): the undo then reads them from a replica it is on now, never takes the
    /// `null`s for no writes (`api::undo::lost_writes`).
    pub(crate) fn restore_point_here(
        &self,
        app_id: &str,
        file: &RestorePointFile,
    ) -> Result<Value, ApiError> {
        let dir = self.restore_points_dir();
        let key = *self.cluster_key();
        let found = crate::backup::find(&dir, &key, app_id, Some(&file.source.name))
            .map_err(|e| unavailable(&format!("the restore point is gone: {e}")))?;
        if found.sha256 != file.source.sha256 {
            return Err(unavailable(
                "the restore point on its node is not the one logged",
            ));
        }
        let (applied, last_write) = self.hosted(app_id).map_or((None, None), |g| {
            (
                Some(g.read_local(|m, _| m.state.last_index)),
                g.read_local(|m, _| m.last_write_ms()),
            )
        });
        Ok(json!({
            "hosted": applied.is_some(),
            "entriesSince": applied.map(|i| i.saturating_sub(file.source.index)),
            "lastWriteMs": last_write,
            "takenAt": found.manifest.as_ref().map(|m| m.written_at),
        }))
    }

    /// [`Self::restore_point_here`] asked of the node that holds `file`.
    pub(crate) async fn restore_point_status(
        &self,
        app_id: &str,
        file: &RestorePointFile,
    ) -> Result<Value, ApiError> {
        if file.node == self.config.node_id {
            return self.restore_point_here(app_id, file);
        }
        let addr = self
            .sys(|s| s.replica_node(&file.node).map(|n| n.addr.clone()))
            .ok_or_else(|| unavailable("the node that holds the restore point is gone"))?;
        let body = json!({ "app": app_id, "op": "check", "file": file });
        self.peer_forward(&addr, "internal/restore-point", &body)
            .await
            .map_err(|e| unavailable(&format!("{e:#}")))
    }

    /// On the node that holds `app_id`'s restore point, once `UndoUpgrade` put the app
    /// in `group` with this node as its seed: seed the group from it (in a task of its
    /// own, so a caller that goes away leaves nothing half done).
    pub(crate) fn seed_undo_here(&self, app_id: &str, group: u64) -> Result<(), ApiError> {
        let file = self
            .sys(|s| {
                s.apps
                    .get(app_id)
                    .filter(|a| a.group == group)
                    .filter(|a| {
                        a.restore.as_ref().and_then(|r| r.seed) == Some(self.config.node_id)
                    })
                    .and_then(|a| a.restore_point.clone())
                    .and_then(|p| p.file.filter(|_| p.undone_at.is_some()))
            })
            .ok_or_else(|| {
                ApiError::conflict("no undo of that app into that group is under way here")
            })?;
        let dir = self.restore_points_dir();
        let key = *self.cluster_key();
        let found = crate::backup::find(&dir, &key, app_id, Some(&file.source.name))
            .map_err(|e| unavailable(&format!("the restore point is gone: {e}")))?;
        if found.sha256 != file.source.sha256 {
            return Err(unavailable(
                "the restore point on its node is not the one logged",
            ));
        }
        let me = self
            .shared()
            .ok_or_else(|| ApiError::unavailable("the node is shutting down"))?;
        let app_id = app_id.to_owned();
        drop(tokio::spawn(async move {
            match me.seed_restore(&app_id, group, &found).await {
                Ok(_) => info!(app = %app_id, group, "undid an upgrade from its restore point"),
                Err(e) => {
                    warn!(app = %app_id, group, error = %e.message, "the undo's restore did not finish; send the undo again")
                }
            }
        }));
        Ok(())
    }

    /// Start seeding `app_id`'s undo into `group` on `seed`, the node that holds its
    /// restore point.
    pub(crate) async fn seed_undo(
        &self,
        app_id: &str,
        group: u64,
        seed: u64,
    ) -> Result<(), ApiError> {
        if seed == self.config.node_id {
            return self.seed_undo_here(app_id, group);
        }
        let addr = self
            .sys(|s| s.replica_node(&seed).map(|n| n.addr.clone()))
            .ok_or_else(|| unavailable("the node that holds the restore point is gone"))?;
        let body = json!({ "app": app_id, "op": "seed", "group": group });
        self.peer_post(&addr, "internal/restore-point", &body)
            .await
            .map(|_| ())
            .map_err(|e| unavailable(&format!("{e:#}")))
    }

    /// Delete the restore points of this node no app's record names, once
    /// [`RESTORE_POINT_GRACE`] old, and those past the undo window. Blocking file work:
    /// run off the async workers.
    pub(crate) fn sweep_restore_points(&self) {
        let dir = self.restore_points_dir();
        let Ok(apps) = std::fs::read_dir(&dir) else {
            return;
        };
        let now = std::time::SystemTime::now();
        let window = Duration::from_secs(UNDO_WINDOW_SECS + 86_400);
        for app_dir in apps.flatten() {
            let app_id = app_dir.file_name().to_string_lossy().into_owned();
            let named = self.sys(|s| {
                s.apps
                    .get(&app_id)
                    .and_then(|a| a.restore_point.as_ref())
                    .and_then(|p| p.file.as_ref())
                    .filter(|f| f.node == self.config.node_id)
                    .map(|f| f.source.name.clone())
            });
            let Ok(files) = std::fs::read_dir(app_dir.path()) else {
                continue;
            };
            let mut left = 0_usize;
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                let snapshot = name.trim_end_matches(crate::backup::MANIFEST_SUFFIX);
                let age = f
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| now.duration_since(t).ok())
                    .unwrap_or_default();
                let keep = if named.as_deref() == Some(snapshot) {
                    age < window
                } else {
                    age < RESTORE_POINT_GRACE
                };
                if keep {
                    left += 1;
                } else if let Err(e) = std::fs::remove_file(f.path()) {
                    warn!(app = %app_id, file = %name, error = %e, "could not drop a restore point");
                }
            }
            if left == 0 {
                let _gone = std::fs::remove_dir(app_dir.path());
            }
        }
    }
}
