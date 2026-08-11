//! Background lifecycle tasks for `ContextManager`.
//!
//! Contains startup recovery (in-progress upgrade propagation) and periodic
//! namespace heartbeat publishing. These are wired in via `Actor::started`.

use actix::{ActorFutureExt, AsyncContext, WrapFuture};
use calimero_context_client::messages::MigrationParams;
use calimero_context_config::types::ContextGroupId;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_store::key::{self, GroupUpgradeStatus};
use calimero_store::Store;

use crate::ContextManager;
use calimero_governance_store::{
    enumerate_group_contexts, MetaRepository, NamespaceRepository, UpgradesRepository,
};

/// The migration for a recovering group's remaining hop, resolved from the
/// apps' embedded ABIs.
///
/// The `InProgress` record cannot answer this: the lazy path persists it
/// synchronously as a concurrency mutex, before an async blob read could
/// resolve the target's ABI, so its `migration` is `None` by construction.
async fn resolve_recovery_migration(
    node_client: &NodeClient,
    datastore: &Store,
    group_id: &ContextGroupId,
    target_application_id: &ApplicationId,
) -> eyre::Result<Option<MigrationParams>> {
    let target_blob = datastore
        .handle()
        .get(&key::ApplicationMeta::new(*target_application_id))?
        .map(|app| *app.bytecode.blob_id().as_ref())
        .ok_or_else(|| eyre::eyre!("target application not found"))?;

    for context_id in enumerate_group_contexts(datastore, group_id, 0, usize::MAX)? {
        // `GroupMeta.app_key` advanced with the `TargetApplicationSet` apply
        // the crash interrupted, so the from-side can only come from what a
        // context actually executes.
        let Some(current) = crate::hlc_fence::loaded_reader_app_key(datastore, &context_id)? else {
            continue;
        };
        if current == target_blob {
            continue;
        }
        // `force_code_only` is not persisted on the record; recovery takes the
        // strict branch.
        return crate::handlers::upgrade_group::resolve_upgrade_from_abis(
            node_client,
            current,
            target_blob,
            false,
        )
        .await;
    }

    // Every context already runs the target bytecode: nothing to migrate.
    Ok(None)
}

impl ContextManager {
    /// Scans the store for in-progress group upgrades and re-spawns
    /// propagators for each. Called during actor startup for crash recovery.
    pub(crate) fn recover_in_progress_upgrades(&mut self, ctx: &mut actix::Context<Self>) {
        let upgrades = match UpgradesRepository::new(&self.datastore).enumerate_in_progress() {
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

            let meta = match MetaRepository::new(&self.datastore).load(&group_id) {
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

            self.active_propagators.insert(group_id);

            let node_client = self.node_client.clone();
            let context_client = self.context_client.clone();
            let datastore = self.datastore.clone();
            let target_application_id = meta.target_application_id;

            let propagator = async move {
                let migration = match resolve_recovery_migration(
                    &node_client,
                    &datastore,
                    &group_id,
                    &target_application_id,
                )
                .await
                {
                    Ok(migration) => migration,
                    // Falling back to `None` would resume a MIGRATING upgrade
                    // as a code-only bytecode swap over un-migrated state. A
                    // record left for an operator is the safe half of that.
                    Err(err) => {
                        tracing::error!(
                            ?group_id, %err,
                            "cannot resolve the migration for an in-progress upgrade; leaving the \
                             record for an operator rather than risking a code-only swap"
                        );
                        return;
                    }
                };

                crate::handlers::upgrade_group::propagate_upgrade(
                    context_client,
                    datastore,
                    group_id,
                    target_application_id,
                    migration,
                )
                .await;
            };

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
                let groups = match MetaRepository::new(&datastore).enumerate_all(0, usize::MAX) {
                    Ok(g) => g,
                    Err(_) => return,
                };

                let namespaces = NamespaceRepository::new(&datastore);
                let mut seen_ns = std::collections::HashSet::new();
                for (group_id_bytes, _meta) in &groups {
                    let gid = ContextGroupId::from(*group_id_bytes);
                    if let Ok(ns_id) = namespaces.resolve(&gid) {
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
