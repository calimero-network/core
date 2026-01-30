//! Calimero node orchestration and coordination.
//!
//! **Purpose**: Main node runtime that coordinates sync, storage, networking, and event handling.
//! **Key Components**:
//! - `NodeManager`: Main actor coordinating all services
//! - `NodeClients`: External service clients (context, node)
//! - `NodeManagers`: Service managers (blobstore, sync)
//! - `NodeState`: Runtime state (caches)

#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

use std::pin::pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::specialized_node_invite_state::{
    new_pending_specialized_node_invites, PendingSpecializedNodeInvites,
};
use actix::{Actor, AsyncContext, WrapFuture};
use calimero_blobstore::BlobManager;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::sync_protocol::{DeltaBuffer, SyncSessionState};
use calimero_primitives::{blobs::BlobId, context::ContextId};
use dashmap::DashMap;
use futures_util::StreamExt;
use tracing::{debug, error, warn};

use crate::delta_store::DeltaStore;

mod arbiter_pool;
mod constants;
mod delta_store;
pub mod gc;
pub mod handlers;
mod run;
mod specialized_node_invite_state;
pub mod sync;
mod utils;

pub use run::{start, NodeConfig, NodeMode, SpecializedNodeConfig};
pub use sync::SyncManager;

/// Cached blob with access tracking for eviction
#[derive(Debug, Clone)]
pub struct CachedBlob {
    pub data: Arc<[u8]>,
    pub last_accessed: Instant,
}

impl CachedBlob {
    pub fn new(data: Arc<[u8]>) -> Self {
        Self {
            data,
            last_accessed: Instant::now(),
        }
    }

    pub fn touch(&mut self) {
        self.last_accessed = Instant::now();
    }
}

/// External service clients (injected dependencies)
#[derive(Debug, Clone)]
pub(crate) struct NodeClients {
    pub(crate) context: ContextClient,
    pub(crate) node: NodeClient,
}

/// Service managers (injected dependencies)
#[derive(Clone, Debug)]
pub(crate) struct NodeManagers {
    pub(crate) blobstore: BlobManager,
    pub(crate) sync: SyncManager,
}

/// Mutable runtime state
#[derive(Clone, Debug)]
pub(crate) struct NodeState {
    pub(crate) blob_cache: Arc<DashMap<BlobId, CachedBlob>>,
    pub(crate) delta_stores: Arc<DashMap<ContextId, DeltaStore>>,
    /// Pending specialized node invites (standard node side) - tracks context_id/inviter for incoming verifications
    pub(crate) pending_specialized_node_invites: PendingSpecializedNodeInvites,
    /// Whether to accept mock TEE attestation (from config, for testing only)
    pub(crate) accept_mock_tee: bool,
    /// Node operation mode (Standard or ReadOnly)
    pub(crate) node_mode: NodeMode,
    /// Sync session state per context (for delta buffering during snapshot sync)
    pub(crate) sync_sessions: Arc<DashMap<ContextId, SyncSession>>,
}

/// Active sync session for a context.
#[derive(Debug)]
pub(crate) struct SyncSession {
    /// Current state of the sync.
    pub(crate) state: SyncSessionState,
    /// Buffer for deltas received during snapshot sync.
    pub(crate) delta_buffer: DeltaBuffer,
}

impl NodeState {
    fn new(accept_mock_tee: bool, node_mode: NodeMode) -> Self {
        Self {
            blob_cache: Arc::new(DashMap::new()),
            delta_stores: Arc::new(DashMap::new()),
            pending_specialized_node_invites: new_pending_specialized_node_invites(),
            accept_mock_tee,
            node_mode,
            sync_sessions: Arc::new(DashMap::new()),
        }
    }

    /// Check if we should buffer a delta (during snapshot sync).
    pub(crate) fn should_buffer_delta(&self, context_id: &ContextId) -> bool {
        self.sync_sessions
            .get(context_id)
            .map_or(false, |session| session.state.should_buffer_deltas())
    }

    /// Buffer a delta during snapshot sync.
    pub(crate) fn buffer_delta(
        &self,
        context_id: &ContextId,
        delta: calimero_node_primitives::sync_protocol::BufferedDelta,
    ) -> bool {
        if let Some(mut session) = self.sync_sessions.get_mut(context_id) {
            session.delta_buffer.push(delta).is_ok()
        } else {
            false
        }
    }

    /// Start a sync session for a context.
    pub(crate) fn start_sync_session(&self, context_id: ContextId, sync_start_hlc: u64) {
        self.sync_sessions.insert(
            context_id,
            SyncSession {
                state: SyncSessionState::BufferingDeltas {
                    buffered_count: 0,
                    sync_start_hlc,
                },
                delta_buffer: DeltaBuffer::new(1000, sync_start_hlc), // Max 1000 buffered deltas
            },
        );
    }

    /// End a sync session and return buffered deltas.
    pub(crate) fn end_sync_session(
        &self,
        context_id: &ContextId,
    ) -> Option<Vec<calimero_node_primitives::sync_protocol::BufferedDelta>> {
        self.sync_sessions
            .remove(context_id)
            .map(|(_, mut session)| session.delta_buffer.drain())
    }

    /// Evict blobs from cache based on age, count, and memory limits
    fn evict_old_blobs(&self) {
        let now = Instant::now();
        let before_count = self.blob_cache.len();

        // Phase 1: Remove blobs older than MAX_BLOB_AGE
        self.blob_cache.retain(|_, cached_blob| {
            now.duration_since(cached_blob.last_accessed)
                < Duration::from_secs(constants::MAX_BLOB_AGE_S)
        });

        let after_time_eviction = self.blob_cache.len();

        // Phase 2: If still over count limit, remove least recently used
        if self.blob_cache.len() > constants::MAX_BLOB_CACHE_COUNT {
            let mut blobs: Vec<_> = self
                .blob_cache
                .iter()
                .map(|entry| (*entry.key(), entry.value().last_accessed))
                .collect();

            // Sort by last_accessed (oldest first)
            blobs.sort_by_key(|(_, accessed)| *accessed);

            // Remove oldest until under count limit
            let to_remove = self.blob_cache.len() - constants::MAX_BLOB_CACHE_COUNT;
            for (blob_id, _) in blobs.iter().take(to_remove) {
                let _removed = self.blob_cache.remove(&blob_id);
            }
        }

        let after_count_eviction = self.blob_cache.len();

        // Phase 3: If still over memory limit, remove by LRU until under budget
        let total_size: usize = self
            .blob_cache
            .iter()
            .map(|entry| entry.value().data.len())
            .sum();

        if total_size > constants::MAX_BLOB_CACHE_SIZE_BYTES {
            let mut blobs: Vec<_> = self
                .blob_cache
                .iter()
                .map(|entry| {
                    (
                        *entry.key(),
                        entry.value().last_accessed,
                        entry.value().data.len(),
                    )
                })
                .collect();

            // Sort by last_accessed (oldest first)
            blobs.sort_by_key(|(_, accessed, _)| *accessed);

            let mut current_size = total_size;
            let mut removed_count = 0;

            for (blob_id, _, size) in blobs {
                if current_size <= constants::MAX_BLOB_CACHE_SIZE_BYTES {
                    break;
                }
                let _removed = self.blob_cache.remove(&blob_id);
                current_size = current_size.saturating_sub(size);
                removed_count += 1;
            }

            if removed_count > 0 {
                #[expect(
                    clippy::integer_division,
                    reason = "MB conversion for logging, precision not critical"
                )]
                let freed_mb = total_size.saturating_sub(current_size) / 1024 / 1024;
                #[expect(
                    clippy::integer_division,
                    reason = "MB conversion for logging, precision not critical"
                )]
                let new_size_mb = current_size / 1024 / 1024;
                tracing::debug!(
                    removed_count,
                    freed_mb,
                    new_size_mb,
                    "Evicted blobs to stay under memory limit"
                );
            }
        }

        let total_evicted = before_count.saturating_sub(self.blob_cache.len());
        if total_evicted > 0 {
            tracing::debug!(
                total_evicted,
                time_evicted = before_count.saturating_sub(after_time_eviction),
                count_evicted = after_time_eviction.saturating_sub(after_count_eviction),
                memory_evicted = after_count_eviction.saturating_sub(self.blob_cache.len()),
                remaining_count = self.blob_cache.len(),
                "Blob cache eviction completed"
            );
        }
    }
}

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

        // Periodic blob cache eviction (every 5 minutes)
        let _handle = ctx.run_interval(
            Duration::from_secs(constants::OLD_BLOBS_EVICTION_FREQUENCY_S),
            |act, _ctx| {
                act.state.evict_old_blobs();
            },
        );

        // Periodic cleanup of stale pending deltas (every 60 seconds)
        let _handle = ctx.run_interval(Duration::from_secs(constants::PENDING_DELTAS_CLEANUP_FREQUENCY_S), |act, ctx| {
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
        });

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
