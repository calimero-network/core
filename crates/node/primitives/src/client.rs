#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use std::borrow::Cow;
// Removed: NonZeroUsize (no longer using height)

use async_stream::stream;
use calimero_blobstore::BlobManager;
use calimero_crypto::SharedKey;
use calimero_network_primitives::client::NetworkClient;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::NodeEvent;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use eyre::{OptionExt, WrapErr};
use futures_util::Stream;
use libp2p::gossipsub::{IdentTopic, TopicHash};
use libp2p::PeerId;
use rand::Rng;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{info, warn};

use crate::messages::NodeMessage;
use crate::sync::BroadcastMessage;

mod alias;
mod application;
mod blob;

/// Result of a sync operation
#[derive(Copy, Clone, Debug)]
pub enum SyncResult {
    /// No sync needed (already in sync)
    NoSyncNeeded,
    /// Delta sync completed
    DeltaSync,
    /// Full resync completed
    FullResync,
}

/// Sync request with optional result channel for waiting
pub type SyncRequest = (
    Option<ContextId>,
    Option<PeerId>,
    Option<oneshot::Sender<eyre::Result<SyncResult>>>,
);

#[derive(Clone, Debug)]
pub struct NodeClient {
    datastore: Store,
    blobstore: BlobManager,
    network_client: NetworkClient,
    node_manager: LazyRecipient<NodeMessage>,
    event_sender: broadcast::Sender<NodeEvent>,
    ctx_sync_tx: mpsc::Sender<SyncRequest>,
}

impl NodeClient {
    #[must_use]
    pub const fn new(
        datastore: Store,
        blobstore: BlobManager,
        network_client: NetworkClient,
        node_manager: LazyRecipient<NodeMessage>,
        event_sender: broadcast::Sender<NodeEvent>,
        ctx_sync_tx: mpsc::Sender<SyncRequest>,
    ) -> Self {
        Self {
            datastore,
            blobstore,
            network_client,
            node_manager,
            event_sender,
            ctx_sync_tx,
        }
    }

    pub async fn subscribe(&self, context_id: &ContextId) -> eyre::Result<()> {
        let topic = IdentTopic::new(context_id);

        let _ignored = self.network_client.subscribe(topic.clone()).await?;

        info!(%context_id, "Subscribed to context");

        Ok(())
    }

    /// Add a locally-created delta to the in-memory DAG.
    ///
    /// This must be called for deltas created by execute() to ensure they're in the DAG
    /// even if gossipsub broadcasts are skipped (no mesh peers yet).
    pub fn add_local_delta(
        &self,
        context_id: ContextId,
        delta_id: [u8; 32],
        parents: Vec<[u8; 32]>,
        actions: Vec<calimero_storage::interface::Action>,
        hlc: calimero_storage::logical_clock::HybridTimestamp,
        expected_root_hash: [u8; 32],
    ) {
        let _ignored = self.node_manager.do_send(NodeMessage::AddLocalDelta {
            context_id,
            delta_id,
            parents,
            actions,
            hlc,
            expected_root_hash,
        });
    }

    pub async fn unsubscribe(&self, context_id: &ContextId) -> eyre::Result<()> {
        let topic = IdentTopic::new(context_id);

        let _ignored = self.network_client.unsubscribe(topic).await?;

        info!(%context_id, "Unsubscribed from context");

        Ok(())
    }

    pub async fn get_peers_count(&self, context: Option<&ContextId>) -> usize {
        let Some(context) = context else {
            return self.network_client.peer_count().await;
        };

        let topic = TopicHash::from_raw(*context);

        self.network_client.mesh_peer_count(topic).await
    }

    pub async fn broadcast(
        &self,
        context: &Context,
        sender: &PublicKey,
        sender_key: &PrivateKey,
        artifact: Vec<u8>,
        delta_id: [u8; 32],
        parent_ids: Vec<[u8; 32]>,
        hlc: calimero_storage::logical_clock::HybridTimestamp,
        events: Option<Vec<u8>>,
    ) -> eyre::Result<()> {
        info!(
            context_id=%context.id,
            %sender,
            root_hash=%context.root_hash,
            delta_id=?delta_id,
            parent_count=parent_ids.len(),
            "Sending state delta"
        );

        // CRITICAL: Add delta to local DeltaStore BEFORE broadcasting
        // This ensures creator has delta in DAG even if broadcast is skipped (no peers)
        // Deserialize actions from artifact
        use calimero_storage::delta::StorageDelta;
        let actions = match borsh::from_slice::<StorageDelta>(&artifact) {
            Ok(StorageDelta::Actions(actions)) => actions,
            _ => {
                warn!(context_id=%context.id, "Failed to deserialize artifact for local delta");
                vec![]
            }
        };

        info!(
            context_id=%context.id,
            delta_id=?delta_id,
            action_count = actions.len(),
            "Calling add_local_delta to store delta in local DAG"
        );

        self.add_local_delta(
            context.id,
            delta_id,
            parent_ids.clone(),
            actions,
            hlc,
            *context.root_hash.as_bytes(),
        );

        // Get mesh peers for diagnostics
        let topic = TopicHash::from_raw(context.id);
        let mesh_peers = self.network_client.mesh_peers(topic.clone()).await;
        let peer_count = mesh_peers.len();

        info!(
            context_id=%context.id,
            mesh_peer_count = peer_count,
            mesh_peers = ?mesh_peers,
            "Gossipsub mesh state before broadcast"
        );

        if peer_count == 0 {
            warn!(context_id=%context.id, "No mesh peers - broadcast skipped (delta already in local DAG)");
            return Ok(());
        }

        let shared_key = SharedKey::from_sk(sender_key);
        let nonce = rand::thread_rng().gen();

        let encrypted = shared_key
            .encrypt(artifact, nonce)
            .ok_or_eyre("failed to encrypt artifact")?;

        let payload = BroadcastMessage::StateDelta {
            context_id: context.id,
            author_id: *sender,
            delta_id,
            parent_ids,
            hlc,
            root_hash: context.root_hash,
            artifact: encrypted.into(),
            nonce,
            events: events.map(Cow::from),
        };

        let payload = borsh::to_vec(&payload)?;

        let _ignored = self.network_client.publish(topic, payload).await?;

        info!(
            context_id=%context.id,
            mesh_peer_count = peer_count,
            "Broadcast published to gossipsub"
        );

        Ok(())
    }

    pub async fn broadcast_heartbeat(
        &self,
        context_id: &ContextId,
        root_hash: calimero_primitives::hash::Hash,
        dag_heads: Vec<[u8; 32]>,
    ) -> eyre::Result<()> {
        if self.get_peers_count(Some(context_id)).await == 0 {
            return Ok(());
        }

        let payload = BroadcastMessage::HashHeartbeat {
            context_id: *context_id,
            root_hash,
            dag_heads,
        };

        let payload = borsh::to_vec(&payload)?;
        let topic = TopicHash::from_raw(*context_id);

        let _ignored = self.network_client.publish(topic, payload).await?;

        Ok(())
    }

    pub fn send_event(&self, event: NodeEvent) -> eyre::Result<()> {
        // the caller doesn't care if there are no receivers
        // so we create a temporary receiver
        let _ignored = self.event_sender.subscribe();

        let _ignored = self
            .event_sender
            .send(event)
            // this should in-theory never happen, but just in case
            .wrap_err("failed to send event")?;

        Ok(())
    }

    pub fn receive_events(&self) -> impl Stream<Item = NodeEvent> {
        let mut receiver = self.event_sender.subscribe();

        stream! {
            loop {
                match receiver.recv().await {
                    Ok(event) => yield event,
                    Err(broadcast::error::RecvError::Closed) => break,
                    // oh, we missed a message? let's.. just ignore it
                    Err(broadcast::error::RecvError::Lagged(_)) => {},
                }
            }
        }
    }

    /// Request a sync operation for a context (fire-and-forget).
    ///
    /// **Non-blocking**: Queues sync request and returns immediately.
    /// Does NOT wait for sync to complete. Use `sync_and_wait()` if you need confirmation.
    ///
    /// **Backpressure**: Returns error immediately if sync queue is full (> 256 pending requests).
    /// This prevents callers from hanging when the system is overloaded.
    ///
    /// **Events**: Listen to `NodeEvent::Sync` events to know when sync completes/fails.
    ///
    /// # Errors
    ///
    /// - `SyncQueueFull`: Too many concurrent sync requests, retry later
    /// - `SyncManagerClosed`: Sync manager has shut down
    pub async fn sync(
        &self,
        context_id: Option<&ContextId>,
        peer_id: Option<&PeerId>,
    ) -> eyre::Result<()> {
        use tokio::sync::mpsc::error::TrySendError;

        match self
            .ctx_sync_tx
            .try_send((context_id.copied(), peer_id.copied(), None))
        {
            Ok(()) => {
                // Instrumentation: Track successful queue operations
                let queue_depth = self.ctx_sync_tx.max_capacity() - self.ctx_sync_tx.capacity();
                if let Some(ctx) = context_id {
                    tracing::debug!(%ctx, queue_depth, "Sync request queued");
                } else {
                    tracing::debug!(queue_depth, "Global sync request queued");
                }
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                // Instrumentation: Track backpressure events
                if let Some(ctx) = context_id {
                    tracing::warn!(
                        %ctx,
                        max_capacity = self.ctx_sync_tx.max_capacity(),
                        "Sync queue full - backpressure applied"
                    );
                } else {
                    tracing::warn!(
                        max_capacity = self.ctx_sync_tx.max_capacity(),
                        "Sync queue full for global sync - backpressure applied"
                    );
                }
                eyre::bail!(
                    "Sync queue full ({} pending requests). System is overloaded, try again later.",
                    self.ctx_sync_tx.max_capacity()
                )
            }
            Err(TrySendError::Closed(_)) => {
                eyre::bail!("Sync manager has shut down")
            }
        }
    }

    /// Synchronize context state and wait for completion (60s timeout).
    ///
    /// Triggers DAG catchup to fetch missing deltas from peers.
    /// Blocks until sync completes or times out.
    pub async fn sync_and_wait(
        &self,
        context_id: Option<&ContextId>,
        peer_id: Option<&PeerId>,
    ) -> eyre::Result<SyncResult> {
        use tokio::sync::mpsc::error::TrySendError;
        use tokio::time::{timeout, Duration};

        let (result_tx, result_rx) = oneshot::channel();

        // Queue sync request with result channel
        match self
            .ctx_sync_tx
            .try_send((context_id.copied(), peer_id.copied(), Some(result_tx)))
        {
            Ok(()) => {
                let queue_depth = self.ctx_sync_tx.max_capacity() - self.ctx_sync_tx.capacity();
                if let Some(ctx) = context_id {
                    tracing::info!(%ctx, queue_depth, "Sync request queued (will wait for completion)");
                } else {
                    tracing::info!(
                        queue_depth,
                        "Global sync request queued (will wait for completion)"
                    );
                }
            }
            Err(TrySendError::Full(_)) => {
                if let Some(ctx) = context_id {
                    tracing::error!(%ctx, "Sync queue full - cannot wait for sync");
                }
                eyre::bail!(
                    "Sync queue full ({} pending requests). System is overloaded.",
                    self.ctx_sync_tx.max_capacity()
                );
            }
            Err(TrySendError::Closed(_)) => {
                eyre::bail!("Sync subsystem has shut down");
            }
        }

        // Wait for sync to complete (with timeout)
        let result = timeout(Duration::from_secs(60), result_rx)
            .await
            .map_err(|_| eyre::eyre!("Sync operation timed out after 60 seconds"))?
            .map_err(|_| eyre::eyre!("Sync result channel closed (sync handler stopped?)"))?;

        if let Some(ctx) = context_id {
            match &result {
                Ok(sync_result) => {
                    tracing::info!(%ctx, ?sync_result, "Sync completed successfully");
                }
                Err(e) => {
                    tracing::error!(%ctx, error = %e, "Sync FAILED");
                }
            }
        }

        result
    }
}
