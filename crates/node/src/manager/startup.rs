use std::pin::pin;
use std::time::Duration;

use actix::{AsyncContext, WrapFuture};
use calimero_context_client::group::ListAllGroupsRequest;
use futures_util::StreamExt;
use tracing::{debug, error, warn};

use super::NodeManager;
use crate::constants;

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
    }

    pub(super) fn setup_maintenance_intervals(&self, ctx: &mut actix::Context<Self>) {
        let _handle = ctx.run_interval(
            Duration::from_secs(constants::OLD_BLOBS_EVICTION_FREQUENCY_S),
            |act, _ctx| {
                act.state.evict_old_blobs();
            },
        );

        let _handle = ctx.run_interval(
            Duration::from_secs(constants::PENDING_DELTAS_CLEANUP_FREQUENCY_S),
            |act, ctx| {
                let max_age = Duration::from_secs(constants::PENDING_DELTA_MAX_AGE_S);
                let delta_stores = act.state.delta_stores_handle();

                let _ignored = ctx.spawn(
                    async move {
                        for entry in delta_stores.iter() {
                            let context_id = *entry.key();
                            let delta_store = entry.value();

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

    pub(super) fn setup_hash_heartbeat_interval(&self, ctx: &mut actix::Context<Self>) {
        let _handle = ctx.run_interval(
            Duration::from_secs(constants::HASH_HEARTBEAT_FREQUENCY_S),
            |act, ctx| {
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
