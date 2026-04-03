use std::pin::pin;
use std::time::Duration;

use actix::{Actor, AsyncContext, WrapFuture};
use calimero_blobstore::BlobManager;
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use futures_util::StreamExt;
use tracing::{debug, error, warn};

use crate::constants;
use crate::run::NodeMode;
use crate::sync::SyncManager;
use crate::{NodeClients, NodeManagers, NodeState};

/// Main node orchestrator.
///
/// **SRP Applied**: Clear separation of:
/// - `clients`: External service clients (context, node)
/// - `managers`: Service managers (blobstore, sync)
/// - `state`: Mutable runtime state (caches)
#[derive(Debug)]
pub struct NodeManager {
    pub(crate) clients: NodeClients,
    pub(crate) managers: NodeManagers,
    pub(crate) state: NodeState,
}

impl NodeManager {
    pub(crate) fn new(
        blobstore: BlobManager,
        sync_manager: SyncManager,
        context_client: ContextClient,
        node_client: NodeClient,
        state: NodeState,
    ) -> Self {
        Self {
            clients: NodeClients {
                context: context_client,
                node: node_client,
            },
            managers: NodeManagers {
                blobstore,
                sync: sync_manager,
            },
            state,
        }
    }
}

impl Actor for NodeManager {
    type Context = actix::Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let node_client = self.clients.node.clone();
        let contexts = self.clients.context.get_context_ids(None);

        // Subscribe to all contexts
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

        // Subscribe to all group topics
        let node_client = self.clients.node.clone();
        let context_client = self.clients.context.clone();

        let _handle = ctx.spawn(
            async move {
                match context_client
                    .list_all_groups(calimero_context_client::group::ListAllGroupsRequest {
                        offset: 0,
                        limit: usize::MAX,
                    })
                    .await
                {
                    Ok(groups) => {
                        for group in groups {
                            if let Err(err) = node_client
                                .subscribe_namespace(group.group_id.to_bytes())
                                .await
                            {
                                error!(?group.group_id, %err, "Failed to subscribe to group topic");
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

        // Periodic blob cache eviction (every 5 minutes)
        let _handle = ctx.run_interval(
            Duration::from_secs(constants::OLD_BLOBS_EVICTION_FREQUENCY_S),
            |act, _ctx| {
                act.state.evict_old_blobs();
            },
        );

        // Periodic cleanup of stale pending deltas (every 60 seconds)
        let _handle = ctx.run_interval(
            Duration::from_secs(constants::PENDING_DELTAS_CLEANUP_FREQUENCY_S),
            |act, ctx| {
                // 5 minutes timeout for pending deltas
                let max_age = Duration::from_secs(constants::PENDING_DELTA_MAX_AGE_S);
                let delta_stores = act.state.delta_stores.clone();

                let _ignored = ctx.spawn(
                    async move {
                        for entry in delta_stores.iter() {
                            let context_id = *entry.key();
                            let delta_store = entry.value();

                            // Evict stale deltas
                            let evicted = delta_store.cleanup_stale(max_age).await;

                            if evicted > 0 {
                                warn!(
                                    %context_id,
                                    evicted_count = evicted,
                                    "Evicted stale pending deltas (timed out after 5 min)"
                                );
                            }

                            // Log stats for monitoring
                            let stats = delta_store.pending_stats().await;
                            if stats.count > 0 {
                                debug!(
                                    %context_id,
                                    pending_count = stats.count,
                                    oldest_age_secs = stats.oldest_age_secs,
                                    missing_parents = stats.total_missing_parents,
                                    "Pending delta statistics"
                                );

                                // Trigger snapshot fallback if too many pending
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

        // Periodic hash heartbeat broadcast (every 30 seconds)
        // Allows peers to detect silent divergence
        let _handle = ctx.run_interval(
            Duration::from_secs(constants::HASH_HEARTBEAT_FREQUENCY_S),
            |act, ctx| {
                let context_client = act.clients.context.clone();
                let node_client = act.clients.node.clone();

                let _ignored = ctx.spawn(
                    async move {
                        // Get all context IDs
                        let contexts = context_client.get_context_ids(None);

                        let mut contexts_stream = pin!(contexts);
                        while let Some(context_id_result) = contexts_stream.next().await {
                            let Ok(context_id) = context_id_result else {
                                continue;
                            };

                            // Get context metadata
                            let Ok(Some(context)) = context_client.get_context(&context_id) else {
                                continue;
                            };

                            // Do not broadcast heartbeat if the node is not initialized.
                            // If the root hash is `[0; 32]` (represented as 1111...1111 in Base58), the node is uninitialized.
                            if context.root_hash.is_zero() {
                                debug!(%context_id, "Skipping heartbeat broadcast: Node uninitialized");
                                continue;
                            }

                            // Broadcast hash heartbeat
                            if let Err(e) = node_client
                                .broadcast_heartbeat(
                                    &context_id,
                                    context.root_hash,
                                    context.dag_heads.clone(),
                                )
                                .await
                            {
                                debug!(
                                    %context_id,
                                    error = %e,
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
