//! Background lifecycle tasks for `ContextManager`.
//!
//! Contains startup recovery (in-progress upgrade propagation), periodic
//! namespace heartbeat publishing and the pending-op sweep. These are wired in
//! via `Actor::started`.

use std::sync::Arc;
use std::time::Duration;

use actix::{ActorFutureExt, AsyncContext, WrapFuture};
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::ContextGroupId;
use calimero_dag::DagStore;
use calimero_store::key::GroupUpgradeStatus;
use tokio::sync::Mutex;

use crate::ContextManager;
use calimero_governance_store::{MetaRepository, NamespaceRepository, UpgradesRepository};

/// How long a namespace op may wait for a missing parent before it is dropped.
const NAMESPACE_PENDING_TTL: Duration = Duration::from_secs(600);

/// How often resident namespace DAGs are swept for ops past the TTL.
const NAMESPACE_PENDING_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// How many dropped op ids one warning lists; the count is always whole.
const MAX_DROPPED_IDS_LOGGED: usize = 8;

/// Drop every pending op older than `ttl` from each namespace's DAG, returning
/// how many went. A DAG busy applying an op is skipped until the next sweep.
///
/// A drop is a namespace falling behind on this node: those ops, and whatever
/// waited on them, are missing until a sync delivers their parents again - and
/// for an op whose parent this node can never authorize (core#4511), never.
/// So each namespace that lost ops is named in a warning, with the ops' ids.
async fn sweep_stale_pending(
    dags: &[([u8; 32], Arc<Mutex<DagStore<SignedNamespaceOp>>>)],
    ttl: Duration,
) -> usize {
    let mut dropped = 0;
    for (namespace_id, dag) in dags {
        let Ok(mut dag) = dag.try_lock() else {
            continue;
        };
        let ids = dag.cleanup_stale_ids(ttl);
        if ids.is_empty() {
            continue;
        }
        dropped += ids.len();
        let sample: Vec<String> = ids
            .iter()
            .take(MAX_DROPPED_IDS_LOGGED)
            .map(hex::encode)
            .collect();
        tracing::warn!(
            namespace_id = %hex::encode(namespace_id),
            dropped = ids.len(),
            ?sample,
            ttl_secs = ttl.as_secs(),
            "dropped namespace ops that waited too long for a parent: their effects \
             are missing on this node until a sync delivers the parents again"
        );
    }
    dropped
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
            let target_application_id = meta.target.application_id;

            let propagator = async move {
                let migration = match crate::handlers::upgrade_group::resolve_resumed_migration(
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
                            "cannot resolve the migration for an in-progress upgrade; refusing to \
                             resume it rather than risk a code-only swap over un-migrated state. \
                             Retrying the upgrade will not clear this (retry needs failed > 0, and \
                             it re-resolves the same way): make the resolution succeed - fetch the \
                             contexts' current bytecode blobs, or rebuild them with an embedded \
                             ABI - then restart the node"
                        );
                        return;
                    }
                };

                crate::handlers::upgrade_group::propagate_upgrade(
                    context_client,
                    node_client,
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

    /// Starts a periodic task that drops namespace ops that have waited for a
    /// missing parent longer than [`NAMESPACE_PENDING_TTL`].
    pub(crate) fn start_namespace_pending_sweep(&self, ctx: &mut actix::Context<Self>) {
        ctx.run_interval(NAMESPACE_PENDING_SWEEP_INTERVAL, |act, _ctx| {
            let dags: Vec<_> = act
                .namespace_dags
                .iter()
                .map(|(id, dag)| (*id, Arc::clone(dag)))
                .collect();
            actix::spawn(async move {
                let _dropped = sweep_stale_pending(&dags, NAMESPACE_PENDING_TTL).await;
            });
        });
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

#[cfg(test)]
mod tests {
    use calimero_dag::{ApplyError, CausalDelta, DeltaApplier};
    use calimero_governance_types::{NamespaceOp, RootOp};
    use calimero_primitives::identity::PrivateKey;

    use super::*;

    struct Accepting;

    #[async_trait::async_trait]
    impl DeltaApplier<SignedNamespaceOp> for Accepting {
        async fn apply(&self, _delta: &CausalDelta<SignedNamespaceOp>) -> Result<(), ApplyError> {
            Ok(())
        }
    }

    /// A DAG holding one op that waits on a parent nobody has.
    async fn dag_with_one_pending_op() -> Arc<Mutex<DagStore<SignedNamespaceOp>>> {
        let signer = PrivateKey::from([0x61; 32]);
        let op = SignedNamespaceOp::sign(
            &signer,
            [0x62; 32].into(),
            vec![[0xEE; 32]],
            1,
            NamespaceOp::Root(RootOp::PolicyUpdated {
                policy_bytes: vec![1],
            }),
        )
        .expect("sign a namespace op");
        let id = op.content_hash().expect("hash the op");
        let mut dag = DagStore::new([0u8; 32]);
        let outcome = dag
            .add_delta_with_outcome(
                CausalDelta::new(
                    id,
                    vec![[0xEE; 32]],
                    op,
                    calimero_storage::logical_clock::HybridTimestamp::default(),
                ),
                &Accepting,
            )
            .await
            .expect("the op is buffered");
        assert!(outcome.is_pending());
        Arc::new(Mutex::new(dag))
    }

    #[tokio::test]
    async fn an_op_that_waited_past_the_ttl_is_dropped() {
        let dag = dag_with_one_pending_op().await;
        tokio::time::sleep(Duration::from_millis(5)).await;

        let dropped =
            sweep_stale_pending(&[([0x5A; 32], Arc::clone(&dag))], Duration::from_millis(1)).await;

        assert_eq!(dropped, 1);
        assert_eq!(dag.lock().await.pending_stats().count, 0);
    }

    #[tokio::test]
    async fn an_op_within_the_ttl_is_kept() {
        let dag = dag_with_one_pending_op().await;

        let dropped =
            sweep_stale_pending(&[([0x5A; 32], Arc::clone(&dag))], Duration::from_secs(3600)).await;

        assert_eq!(dropped, 0);
        assert_eq!(dag.lock().await.pending_stats().count, 1);
    }

    #[tokio::test]
    async fn a_dag_busy_applying_is_left_for_the_next_sweep() {
        let dag = dag_with_one_pending_op().await;
        tokio::time::sleep(Duration::from_millis(5)).await;

        let held = dag.lock().await;
        let dropped =
            sweep_stale_pending(&[([0x5A; 32], Arc::clone(&dag))], Duration::from_millis(1)).await;
        drop(held);

        assert_eq!(dropped, 0, "the sweep must not wait on the DAG lock");
        assert_eq!(dag.lock().await.pending_stats().count, 1);
    }
}
