use std::pin::pin;
use std::time::Duration;

use actix::{AsyncContext, WrapFuture};
use calimero_context_client::group::ListAllGroupsRequest;
use calimero_governance_store::NodeDeviceRepository;
use calimero_primitives::context::ContextId;
use futures_util::StreamExt;
use tracing::{debug, error, warn};

use super::NodeManager;
use crate::constants;
use crate::delta_store::DeltaStore;

impl NodeManager {
    pub(super) fn setup_startup_subscriptions(&self, ctx: &mut actix::Context<Self>) {
        let node_client = self.clients.node.clone();
        let contexts = self.clients.context.get_context_ids(None);

        let _handle = ctx.spawn(
            async move {
                let mut contexts = pin!(contexts);
                while let Some(context_id) = contexts.next().await {
                    let Ok(context_id) = context_id else {
                        error!("Failed to get context ID");
                        continue;
                    };

                    if let Err(err) = node_client.subscribe(&context_id).await {
                        error!(%context_id, %err, "Failed to subscribe to context");
                    }
                }
            }
            .into_actor(self),
        );

        let node_client = self.clients.node.clone();
        let context_client = self.clients.context.clone();

        let _handle = ctx.spawn(
            async move {
                match context_client
                    .list_all_groups(ListAllGroupsRequest {
                        offset: 0,
                        limit: usize::MAX,
                    })
                    .await
                {
                    Ok(groups) => {
                        for group in groups {
                            let ns_bytes = group.group_id.to_bytes();
                            if let Err(err) = node_client.subscribe_namespace(ns_bytes).await {
                                error!(?group.group_id, %err, "Failed to subscribe to group topic");
                                continue;
                            }
                            // Pull governance ops directly from a live peer.
                            // This catches any ops we missed while offline.
                            // sync_namespace_from_peer returns silently if no
                            // mesh peers are up yet; the heartbeat interval
                            // will retry on subsequent ticks.
                            if let Err(err) = node_client.sync_namespace(ns_bytes).await {
                                warn!(?group.group_id, %err, "Failed to queue namespace governance sync at startup");
                            }
                        }
                    }
                    Err(err) => {
                        error!(%err, "Failed to list groups for startup subscription");
                    }
                }
            }
            .into_actor(self),
        );

        // Namespaces this node holds a *device* in, which is not the same set as
        // the one above. `list_all_groups` filters on membership, and a paired
        // device is deliberately a member of nothing — it is one device of an
        // account that belongs to somebody else. Without this loop such a node
        // comes back from a restart subscribed to no topic at all: it keeps its
        // identity and its device secret, so nothing errors and nothing logs, it
        // just silently stops receiving the ops it is still entitled to author
        // against.
        //
        // The two loops overlap for an ordinary member that has enrolled a
        // device, and that is fine — `subscribe_namespace` is idempotent.
        let node_client = self.clients.node.clone();
        let datastore = self.datastore.clone();

        let _handle = ctx.spawn(
            async move {
                let namespaces = match NodeDeviceRepository::new(&datastore).enrolled_namespaces() {
                    Ok(namespaces) => namespaces,
                    Err(err) => {
                        error!(%err, "Failed to list enrolled devices for startup subscription");
                        return;
                    }
                };
                for namespace in namespaces {
                    let ns_bytes = namespace.to_bytes();
                    if let Err(err) = node_client.subscribe_namespace(ns_bytes).await {
                        error!(?namespace, %err, "Failed to subscribe to device namespace topic");
                        continue;
                    }
                    if let Err(err) = node_client.sync_namespace(ns_bytes).await {
                        warn!(?namespace, %err, "Failed to queue device namespace governance sync at startup");
                    }
                }
            }
            .into_actor(self),
        );
    }

    pub(super) fn setup_maintenance_intervals(&self, ctx: &mut actix::Context<Self>) {
        let _handle = ctx.run_interval(
            Duration::from_secs(constants::OLD_BLOBS_EVICTION_FREQUENCY_S),
            |act, _ctx| {
                act.state.evict_old_blobs();
            },
        );

        // Periodic gossipsub mesh-peer-count snapshot. Logs one entry per
        // subscribed topic so CI / operators can see the actual mesh size,
        // independent of the libp2p-gossipsub internal `Updating mesh,
        // new mesh: {…}` heartbeat log (which reports additions, not
        // current state, and is easy to misread as "mesh is empty" when
        // the mesh has simply already been populated).
        let _handle = ctx.run_interval(
            Duration::from_secs(constants::MESH_STATS_LOG_FREQUENCY_S),
            |act, ctx| {
                let network_client = act.clients.node.network_client().clone();

                let _ignored = ctx.spawn(
                    async move {
                        let stats = network_client.mesh_stats().await;
                        if stats.is_empty() {
                            debug!("gossipsub mesh: no subscribed topics");
                            return;
                        }
                        let total: usize = stats.iter().map(|(_, n)| *n).sum();
                        let topics = stats.len();
                        for (topic, count) in &stats {
                            debug!(%topic, mesh_peers = count, "gossipsub mesh size");
                        }
                        debug!(topics, total_mesh_peers = total, "gossipsub mesh summary");
                    }
                    .into_actor(act),
                );
            },
        );

        let _handle = ctx.run_interval(
            Duration::from_secs(constants::PENDING_DELTAS_CLEANUP_FREQUENCY_S),
            |act, ctx| {
                let max_age = Duration::from_secs(constants::PENDING_DELTA_MAX_AGE_S);
                let delta_stores = act.state.delta_stores_handle();

                // Snapshot (context_id, DeltaStore) pairs and drop the DashMap
                // iterator BEFORE any `.await`. Iterating a `DashMap` holds a
                // shard read-guard for the lifetime of each `RefMulti`; awaiting
                // `cleanup_stale`/`pending_stats` (which take DAG locks) while
                // holding that guard would block new-context registration landing
                // on the same shard for the whole cleanup. `DeltaStore` is a cheap
                // `Arc`-backed clone, so the snapshot is a shallow copy of handles.
                let snapshot: Vec<(ContextId, DeltaStore)> = delta_stores
                    .iter()
                    .map(|entry| (*entry.key(), entry.value().clone()))
                    .collect();
                drop(delta_stores);

                let _ignored = ctx.spawn(
                    async move {
                        for (context_id, delta_store) in snapshot {
                            let evicted = delta_store.cleanup_stale(max_age).await;
                            if evicted > 0 {
                                warn!(
                                    %context_id,
                                    evicted_count = evicted,
                                    "Evicted stale pending deltas (timed out after 5 min)"
                                );
                            }

                            let stats = delta_store.pending_stats().await;
                            if stats.count > 0 {
                                debug!(
                                    %context_id,
                                    pending_count = stats.count,
                                    oldest_age_secs = stats.oldest_age_secs,
                                    missing_parents = stats.total_missing_parents,
                                    "Pending delta statistics"
                                );

                                if stats.count > constants::PENDING_DELTA_SNAPSHOT_THRESHOLD {
                                    warn!(
                                        %context_id,
                                        pending_count = stats.count,
                                        threshold = constants::PENDING_DELTA_SNAPSHOT_THRESHOLD,
                                        "Too many pending deltas - state sync will recover on next periodic sync"
                                    );
                                }
                            }
                        }
                    }
                    .into_actor(act),
                );
            },
        );
    }

    /// Schedule the ephemeral-presence heartbeat: every
    /// [`PRESENCE_HEARTBEAT_MS`] milliseconds, re-publish all locally-set
    /// ephemeral slices (with bumped seq so remote nodes refresh liveness)
    /// and sweep stale remote entries from the [`AwarenessStore`].
    ///
    /// Mirrors the `setup_hash_heartbeat_interval` pattern; uses
    /// [`heartbeat_tick`] from the outbound ephemeral handler.
    ///
    /// [`PRESENCE_HEARTBEAT_MS`]: crate::handlers::ephemeral::PRESENCE_HEARTBEAT_MS
    /// [`AwarenessStore`]: crate::handlers::ephemeral::store::AwarenessStore
    /// [`heartbeat_tick`]: crate::handlers::ephemeral::outbound::heartbeat_tick
    pub(super) fn setup_ephemeral_heartbeat_interval(&self, ctx: &mut actix::Context<Self>) {
        use crate::handlers::ephemeral::outbound::heartbeat_tick;
        use crate::handlers::ephemeral::PRESENCE_HEARTBEAT_MS;

        let _handle = ctx.run_interval(Duration::from_millis(PRESENCE_HEARTBEAT_MS), |act, ctx| {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            heartbeat_tick(act, ctx, now_ms);
        });
    }

    pub(super) fn setup_hash_heartbeat_interval(&self, ctx: &mut actix::Context<Self>) {
        let _handle = ctx.run_interval(
            Duration::from_secs(constants::HASH_HEARTBEAT_FREQUENCY_S),
            |act, ctx| {
                // Reclaim heartbeat bookkeeping for peers/contexts that have gone
                // quiet, so these maps can't grow without bound on peer churn.
                // Both are touched only here and in the (synchronous) heartbeat
                // handler on this actor, so a plain retain is race-free.
                let now = std::time::Instant::now();
                act.divergence_streak
                    .retain(|_, mark| now.duration_since(mark.last_seen) < crate::manager::HEARTBEAT_STATE_TTL);
                act.behind_sync_at
                    .retain(|_, last| now.duration_since(*last) < crate::manager::HEARTBEAT_STATE_TTL);

                let context_client = act.clients.context.clone();
                let node_client = act.clients.node.clone();

                let _ignored = ctx.spawn(
                    async move {
                        let contexts = context_client.get_context_ids(None);
                        let mut contexts_stream = pin!(contexts);
                        while let Some(context_id_result) = contexts_stream.next().await {
                            let Ok(context_id) = context_id_result else {
                                continue;
                            };

                            let Ok(Some(context)) = context_client.get_context(&context_id) else {
                                continue;
                            };

                            if context.root_hash.is_zero() {
                                debug!(%context_id, "Skipping heartbeat broadcast: Node uninitialized");
                                continue;
                            }

                            if let Err(err) = node_client
                                .broadcast_heartbeat(
                                    &context_id,
                                    context.root_hash,
                                    context.dag_heads.clone(),
                                )
                                .await
                            {
                                debug!(
                                    %context_id,
                                    error = %err,
                                    "Failed to broadcast hash heartbeat"
                                );
                            }
                        }
                    }
                    .into_actor(act),
                );
            },
        );
    }
}
