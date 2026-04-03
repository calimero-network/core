//! Background lifecycle tasks for `ContextManager`.
//!
//! Contains startup recovery (in-progress upgrade propagation) and periodic
//! namespace heartbeat publishing. These are wired in via `Actor::started`.

use actix::{ActorFutureExt, AsyncContext, WrapFuture};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::UpgradePolicy;
use calimero_store::key::GroupUpgradeStatus;

use crate::group_store;
use crate::ContextManager;

impl ContextManager {
    /// Scans the store for in-progress group upgrades and re-spawns
    /// propagators for each. Called during actor startup for crash recovery.
    pub(crate) fn recover_in_progress_upgrades(&mut self, ctx: &mut actix::Context<Self>) {
        let upgrades = match group_store::enumerate_in_progress_upgrades(&self.datastore) {
            Ok(u) => u,
            Err(err) => {
                tracing::error!(
                    ?err,
                    "failed to scan for in-progress upgrades during recovery"
                );
                return;
            }
        };

        if upgrades.is_empty() {
            return;
        }

        tracing::info!(
            count = upgrades.len(),
            "recovering in-progress group upgrades"
        );

        for (group_id, upgrade) in upgrades {
            let (total, completed, failed) = match upgrade.status {
                GroupUpgradeStatus::InProgress {
                    total,
                    completed,
                    failed,
                } => (total, completed, failed),
                _ => continue,
            };

            tracing::info!(
                ?group_id,
                total,
                completed,
                failed,
                "re-spawning propagator for in-progress upgrade"
            );

            let migration = upgrade
                .migration
                .as_ref()
                .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
                .map(|method| calimero_context_primitives::messages::MigrationParams { method });

            let meta = match group_store::load_group_meta(&self.datastore, &group_id) {
                Ok(Some(m)) => m,
                Ok(None) => {
                    tracing::warn!(?group_id, "group not found during recovery, skipping");
                    continue;
                }
                Err(err) => {
                    tracing::error!(?group_id, ?err, "failed to load group meta during recovery");
                    continue;
                }
            };

            if matches!(meta.upgrade_policy, UpgradePolicy::LazyOnAccess) {
                tracing::debug!(?group_id, "skipping crash recovery for LazyOnAccess group");
                continue;
            }

            self.active_propagators.insert(group_id);

            let propagator = crate::handlers::upgrade_group::propagate_upgrade(
                self.context_client.clone(),
                self.datastore.clone(),
                group_id,
                meta.target_application_id,
                migration,
                None,
                0,
            );

            ctx.spawn(propagator.into_actor(self).map(move |_, act, _| {
                act.active_propagators.remove(&group_id);
            }));
        }
    }

    /// Starts a periodic task that publishes namespace governance heartbeats.
    ///
    /// Every 30 seconds, iterates all known groups, collects unique namespaces,
    /// and publishes the current DAG heads as a heartbeat for peer discovery.
    pub(crate) fn start_namespace_heartbeat(&self, ctx: &mut actix::Context<Self>) {
        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();

        ctx.run_interval(std::time::Duration::from_secs(30), move |_act, _ctx| {
            let datastore = datastore.clone();
            let node_client = node_client.clone();

            actix::spawn(async move {
                let groups = match group_store::enumerate_all_groups(&datastore, 0, usize::MAX) {
                    Ok(g) => g,
                    Err(_) => return,
                };

                let mut seen_ns = std::collections::HashSet::new();
                for (group_id_bytes, _meta) in &groups {
                    let gid = ContextGroupId::from(*group_id_bytes);
                    if let Ok(ns_id) = group_store::resolve_namespace(&datastore, &gid) {
                        let ns_bytes = ns_id.to_bytes();
                        if !seen_ns.insert(ns_bytes) {
                            continue;
                        }
                        let handle = datastore.handle();
                        let ns_key = calimero_store::key::NamespaceGovHead::new(ns_bytes);
                        if let Ok(Some(head)) = handle.get(&ns_key) {
                            let _ = node_client
                                .publish_namespace_heartbeat(ns_bytes, head.dag_heads)
                                .await;
                        }
                    }
                }
            });
        });
    }
}
