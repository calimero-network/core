#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
// Removed: NonZeroUsize (no longer using height)

use async_stream::stream;
use calimero_crypto::SharedKey;
use calimero_network_primitives::client::NetworkClient;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::NodeEvent;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use dashmap::DashMap;
use eyre::{OptionExt, WrapErr};
use futures_util::Stream;
use libp2p::gossipsub::{IdentTopic, TopicHash};
use libp2p::PeerId;
use rand::Rng;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;
use tokio::sync::oneshot;

use crate::messages::{
    NodeMessage, RegisterPendingSpecializedNodeInvite, RemovePendingSpecializedNodeInvite,
};
use crate::sync::{BroadcastMessage, MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES};
use crate::TopicManager;

pub use crate::join_bundle::JoinBundle;

mod alias;
mod application;
mod blob;

pub use blob::BlobManager;

/// Parameters for a direct namespace join request.
#[derive(Debug)]
pub struct NamespaceJoinParams {
    pub namespace_id: [u8; 32],
    pub invitation_bytes: Vec<u8>,
    pub joiner_public_key: PublicKey,
}

#[derive(Clone, Debug)]
pub struct SyncClient {
    ctx_sync_tx: mpsc::Sender<(Option<ContextId>, Option<PeerId>)>,
    ns_sync_tx: mpsc::Sender<[u8; 32]>,
    ns_join_tx: mpsc::Sender<(
        NamespaceJoinParams,
        oneshot::Sender<eyre::Result<JoinBundle>>,
    )>,
}

impl SyncClient {
    #[must_use]
    pub fn new(
        ctx_sync_tx: mpsc::Sender<(Option<ContextId>, Option<PeerId>)>,
        ns_sync_tx: mpsc::Sender<[u8; 32]>,
        ns_join_tx: mpsc::Sender<(
            NamespaceJoinParams,
            oneshot::Sender<eyre::Result<JoinBundle>>,
        )>,
    ) -> Self {
        Self {
            ctx_sync_tx,
            ns_sync_tx,
            ns_join_tx,
        }
    }

    pub async fn sync(
        &self,
        context_id: Option<&ContextId>,
        peer_id: Option<&PeerId>,
    ) -> eyre::Result<()> {
        self.ctx_sync_tx
            .send((context_id.copied(), peer_id.copied()))
            .await?;

        Ok(())
    }

    /// Request a full namespace governance sync from any available peer.
    ///
    /// Opens a stream to a mesh peer on the namespace topic and pulls
    /// all governance ops via `NamespaceBackfillRequest`. The ops are
    /// applied locally via `apply_signed_namespace_op`.
    pub async fn sync_namespace(&self, namespace_id: [u8; 32]) -> eyre::Result<()> {
        self.ns_sync_tx.send(namespace_id).await?;
        Ok(())
    }

    /// Send a direct namespace join request to a mesh peer and await the
    /// response. The SyncManager handles the actual stream I/O.
    pub async fn request_namespace_join(
        &self,
        namespace_id: [u8; 32],
        invitation_bytes: Vec<u8>,
        joiner_public_key: PublicKey,
    ) -> eyre::Result<JoinBundle> {
        let (tx, rx) = oneshot::channel();
        let params = NamespaceJoinParams {
            namespace_id,
            invitation_bytes,
            joiner_public_key,
        };
        self.ns_join_tx
            .send((params, tx))
            .await
            .map_err(|_| eyre::eyre!("namespace join channel closed"))?;
        rx.await
            .map_err(|_| eyre::eyre!("namespace join response channel dropped"))?
    }
}

/// Notification payload sent by the execute path after a locally-created
/// delta has been persisted to the DB, so the node-side can update its
/// in-memory `DeltaStore` incrementally instead of re-scanning the DB on
/// every interval sync. Kept as plain types so this crate doesn't need
/// a direct dependency on `calimero-dag` — the receiver reconstructs
/// `CausalDelta` internally.
#[derive(Debug)]
pub struct LocalAppliedDelta {
    pub context_id: ContextId,
    pub delta_id: [u8; 32],
    pub parents: Vec<[u8; 32]>,
    pub hlc: calimero_storage::logical_clock::HybridTimestamp,
    pub expected_root_hash: [u8; 32],
    pub actions: Vec<calimero_storage::action::Action>,
}

/// Read libp2p's `mesh_n_low` once from the live `gossipsub::Config::default()`
/// and cache it. Used by Phase-1 readiness in
/// `governance_broadcast::assert_transport_ready` as the upper bound for
/// `required = min(mesh_n_low, known_subscribers)`.
///
/// Reading from `Config::default()` (instead of hardcoding) keeps this
/// value in sync across libp2p version bumps — the upstream default has
/// shifted between releases (4 → 5 between older crates and the 0.49.x
/// line currently pinned), and a hardcoded mismatch would either reject
/// healthy publishes (`required` too high) or admit publishes on an
/// unhealthy mesh (`required` too low). Calimero constructs the
/// gossipsub behaviour with `Config::default()` at
/// `crates/network/src/behaviour.rs:111`, so reading the same default
/// here is faithful to the actor's configuration.
fn gossipsub_mesh_n_low_default() -> usize {
    static CACHED: OnceLock<usize> = OnceLock::new();
    *CACHED.get_or_init(|| libp2p::gossipsub::Config::default().mesh_n_low())
}

#[derive(Clone, Debug)]
pub struct NodeClient {
    datastore: Store,
    blob_manager: BlobManager,
    network_client: NetworkClient,
    topic_manager: TopicManager,
    node_manager: LazyRecipient<NodeMessage>,
    event_sender: broadcast::Sender<NodeEvent>,
    sync_client: SyncClient,
    specialized_node_invite_topic: String,
    /// Channel for notifying the node about locally-applied deltas so
    /// its in-memory `DeltaStore` stays in sync without re-scanning the
    /// DB each `perform_interval_sync`. `None` in unit/integration
    /// tests that construct `NodeClient` without a running node.
    local_delta_tx: Option<mpsc::Sender<LocalAppliedDelta>>,
    /// Per-topic set of remote peers we've observed `Subscribed` to,
    /// minus those we've subsequently observed `Unsubscribed`. Populated
    /// by `subscriptions::handle_subscribed/unsubscribed` in the node
    /// crate; queried by `governance_broadcast::assert_transport_ready`
    /// on the publish path. Shared by `Arc<DashMap>` so the writer
    /// (NodeManager event handler) and readers (concurrent publishers)
    /// see the same map without an actor mailbox round-trip.
    known_subscribers: Arc<DashMap<TopicHash, HashSet<PeerId>>>,
}

impl NodeClient {
    #[must_use]
    pub fn new(
        datastore: Store,
        blob_manager: BlobManager,
        network_client: NetworkClient,
        node_manager: LazyRecipient<NodeMessage>,
        event_sender: broadcast::Sender<NodeEvent>,
        sync_client: SyncClient,
        specialized_node_invite_topic: String,
        local_delta_tx: Option<mpsc::Sender<LocalAppliedDelta>>,
    ) -> Self {
        let topic_manager = TopicManager::new(network_client.clone());
        Self {
            datastore,
            blob_manager,
            network_client,
            topic_manager,
            node_manager,
            event_sender,
            sync_client,
            specialized_node_invite_topic,
            local_delta_tx,
            known_subscribers: Arc::new(DashMap::new()),
        }
    }

    /// Record that `peer_id` subscribed to `topic`. Called from the
    /// gossipsub `Subscribed` event handler. Idempotent: re-subscriptions
    /// are deduped by the per-topic `HashSet`.
    pub fn record_peer_subscribed(&self, peer_id: PeerId, topic: TopicHash) {
        let _new = self
            .known_subscribers
            .entry(topic)
            .or_default()
            .insert(peer_id);
    }

    /// Record that `peer_id` unsubscribed from `topic`. The map entry is
    /// removed once its set goes empty so [`known_subscribers`](Self::known_subscribers)
    /// returns 0 instead of an empty-set marker — Phase-1 readiness
    /// treats both identically, but the cleanup keeps the map bounded.
    ///
    /// The set-mutation and the empty-entry cleanup are split into two
    /// shard-lock acquisitions, but the cleanup uses [`DashMap::remove_if`]
    /// so a concurrent `record_peer_subscribed` for the same topic
    /// arriving between them cannot have its insertion silently erased —
    /// `remove_if` re-checks emptiness atomically inside the shard lock.
    pub fn record_peer_unsubscribed(&self, peer_id: &PeerId, topic: &TopicHash) {
        if let Some(mut set) = self.known_subscribers.get_mut(topic) {
            let _ = set.remove(peer_id);
        }
        let _ = self
            .known_subscribers
            .remove_if(topic, |_, set| set.is_empty());
    }

    /// Number of distinct remote peers currently observed subscribed to
    /// `topic` (NOT mesh members — subscription is the strict superset).
    /// Used by Phase-1 governance readiness to cap the required mesh
    /// quorum: a 2-node namespace cannot reach `mesh_n_low` regardless,
    /// so the readiness gate must be aware of the population size.
    pub fn known_subscribers(&self, topic: &TopicHash) -> usize {
        self.known_subscribers
            .get(topic)
            .map(|set| set.len())
            .unwrap_or(0)
    }

    /// Gossipsub `mesh_n_low` — see [`gossipsub_mesh_n_low_default`].
    #[must_use]
    pub fn gossipsub_mesh_n_low(&self) -> usize {
        gossipsub_mesh_n_low_default()
    }

    /// Borrow the underlying `NetworkClient`. Used by
    /// `governance_broadcast::publish_and_await_ack_*` to plug the
    /// transport into the helper's `BroadcastTransport` trait, avoiding
    /// a redundant clone on every publish.
    #[must_use]
    pub fn network_client(&self) -> &NetworkClient {
        &self.network_client
    }

    /// Notify the node that a locally-applied delta was just persisted to
    /// the DB. The node's `DeltaStore` will add it to the in-memory DAG
    /// asynchronously, removing the need for periodic `load_persisted_
    /// deltas` rescans on the sync hot path.
    ///
    /// Best-effort: if the channel is closed (node shutting down) or
    /// full, logs and moves on. Worst case on restart,
    /// `load_persisted_deltas` catches up at startup.
    pub fn notify_local_applied_delta(&self, delta: LocalAppliedDelta) {
        let Some(tx) = self.local_delta_tx.as_ref() else {
            return;
        };
        if let Err(err) = tx.try_send(delta) {
            warn!(
                ?err,
                "failed to enqueue local applied delta for in-memory DAG update — \
                 next startup will recover via load_persisted_deltas"
            );
        }
    }

    pub async fn subscribe(&self, context_id: &ContextId) -> eyre::Result<()> {
        let topic = String::from(context_id);
        self.topic_manager.ensure_subscribed(&topic).await?;
        info!(%context_id, "Subscribed to context");
        Ok(())
    }

    pub async fn unsubscribe(&self, context_id: &ContextId) -> eyre::Result<()> {
        let topic = String::from(context_id);
        self.topic_manager.unsubscribe(&topic).await?;
        info!(%context_id, "Unsubscribed from context");
        Ok(())
    }

    /// Subscribe to the namespace governance topic `ns/<hex(namespace_id)>`.
    pub async fn subscribe_namespace(&self, namespace_id: [u8; 32]) -> eyre::Result<()> {
        let topic = format!("ns/{}", hex::encode(namespace_id));
        self.topic_manager.ensure_subscribed(&topic).await?;
        info!(
            namespace_id = %hex::encode(namespace_id),
            "Subscribed to namespace topic"
        );
        Ok(())
    }

    /// Unsubscribe from the namespace governance topic.
    pub async fn unsubscribe_namespace(&self, namespace_id: [u8; 32]) -> eyre::Result<()> {
        let topic = format!("ns/{}", hex::encode(namespace_id));
        self.topic_manager.unsubscribe(&topic).await?;
        info!(
            namespace_id = %hex::encode(namespace_id),
            "Unsubscribed from namespace topic"
        );
        Ok(())
    }

    /// Publish raw payload on the namespace topic `ns/<hex(namespace_id)>`.
    pub async fn publish_on_namespace(
        &self,
        namespace_id: [u8; 32],
        payload: Vec<u8>,
    ) -> eyre::Result<()> {
        let topic_str = format!("ns/{}", hex::encode(namespace_id));
        let topic = TopicHash::from_raw(topic_str);

        const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

        let deadline = tokio::time::Instant::now() + MAX_WAIT;
        loop {
            let peers = self.network_client.mesh_peer_count(topic.clone()).await;
            if peers > 0 {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                warn!(
                    ?namespace_id,
                    "no mesh peers after {MAX_WAIT:?}, publishing anyway"
                );
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        let _ignored = self.network_client.publish(topic, payload).await?;
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
        governance_epoch: Vec<[u8; 32]>,
        key_id: [u8; 32],
    ) -> eyre::Result<()> {
        info!(
            context_id=%context.id,
            %sender,
            root_hash=%context.root_hash,
            delta_id=?delta_id,
            parent_count=parent_ids.len(),
            governance_epoch_len=governance_epoch.len(),
            "Sending state delta"
        );

        if self.get_peers_count(Some(&context.id)).await == 0 {
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
            governance_epoch,
            key_id,
        };

        let payload = borsh::to_vec(&payload)?;

        let topic = TopicHash::from_raw(context.id);

        let _ignored = self.network_client.publish(topic, payload).await?;

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

    /// Mesh peer count for the namespace topic `ns/<hex>` — used by callers
    /// (governance publish sites) to observe `governance_publish_mesh_peers_at_publish`.
    pub async fn mesh_peer_count_for_namespace(&self, namespace_id: [u8; 32]) -> usize {
        let topic_str = format!("ns/{}", hex::encode(namespace_id));
        let topic = TopicHash::from_raw(topic_str);
        self.network_client.mesh_peer_count(topic).await
    }

    /// Publish a borsh-encoded `SignedNamespaceOp` on the namespace topic `ns/<hex>`.
    ///
    /// Enforces [`MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES`] on the payload.
    pub async fn publish_signed_namespace_op(
        &self,
        namespace_id: [u8; 32],
        delta_id: [u8; 32],
        parent_ids: Vec<[u8; 32]>,
        signed_op_borsh: Vec<u8>,
    ) -> eyre::Result<()> {
        if signed_op_borsh.len() > MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES {
            eyre::bail!(
                "signed namespace op payload exceeds max ({} > {})",
                signed_op_borsh.len(),
                MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES
            );
        }

        let topic_str = format!("ns/{}", hex::encode(namespace_id));
        let topic = TopicHash::from_raw(topic_str);

        let peers = self.network_client.mesh_peer_count(topic.clone()).await;
        if peers == 0 {
            warn!(
                namespace_id = %hex::encode(namespace_id),
                "no mesh peers on namespace topic, governance op publish is best-effort"
            );
        }

        let payload = BroadcastMessage::NamespaceGovernanceDelta {
            namespace_id,
            delta_id,
            parent_ids,
            payload: signed_op_borsh,
        };
        let payload_bytes = borsh::to_vec(&payload)?;

        if let Err(err) = self.network_client.publish(topic, payload_bytes).await {
            warn!(
                namespace_id = %hex::encode(namespace_id),
                %err,
                "failed to publish signed namespace op"
            );
        }

        Ok(())
    }

    /// Publish a namespace governance heartbeat for DAG divergence detection.
    pub async fn publish_namespace_heartbeat(
        &self,
        namespace_id: [u8; 32],
        dag_heads: Vec<[u8; 32]>,
    ) -> eyre::Result<()> {
        let topic_str = format!("ns/{}", hex::encode(namespace_id));
        let topic = TopicHash::from_raw(topic_str);

        let payload = BroadcastMessage::NamespaceStateHeartbeat {
            namespace_id,
            dag_heads,
        };
        let payload_bytes = borsh::to_vec(&payload)?;
        if let Err(err) = self.network_client.publish(topic, payload_bytes).await {
            debug!(
                namespace_id = %hex::encode(namespace_id),
                %err,
                "failed to publish namespace heartbeat"
            );
        }
        Ok(())
    }

    /// Broadcast a specialized node invite discovery to the global invite topic.
    ///
    /// This broadcasts a discovery message and registers a pending invite so that
    /// when a specialized node responds with verification, the node can create an invitation.
    ///
    /// # Arguments
    /// * `context_id` - The context to invite specialized nodes to
    /// * `inviter_id` - The identity performing the invitation
    /// * `invite_topic` - The global topic name for specialized node invite discovery
    ///
    /// # Returns
    /// The nonce used in the request
    pub async fn broadcast_specialized_node_invite(
        &self,
        context_id: ContextId,
        inviter_id: PublicKey,
    ) -> eyre::Result<[u8; 32]> {
        let nonce: [u8; 32] = rand::thread_rng().gen();
        // Currently only ReadOnly node type is supported
        let node_type = SpecializedNodeType::ReadOnly;

        info!(
            %context_id,
            %inviter_id,
            ?node_type,
            topic = %self.specialized_node_invite_topic,
            nonce = %hex::encode(nonce),
            "Broadcasting specialized node invite discovery"
        );

        // Register the pending invite FIRST to avoid race condition
        // A fast-responding specialized node could send verification request
        // before registration completes if we broadcast first
        self.node_manager
            .send(NodeMessage::RegisterPendingSpecializedNodeInvite {
                request: RegisterPendingSpecializedNodeInvite {
                    nonce,
                    context_id,
                    inviter_id,
                },
            })
            .await
            .expect("Mailbox not to be dropped");

        // Now broadcast the discovery message
        let payload = BroadcastMessage::SpecializedNodeDiscovery { nonce, node_type };
        let payload = borsh::to_vec(&payload)?;
        let topic = IdentTopic::new(self.specialized_node_invite_topic.to_owned());
        let result = self.network_client.publish(topic.hash(), payload).await;

        // If broadcast failed, clean up the pending invite before returning error
        if result.is_err() {
            self.node_manager
                .send(NodeMessage::RemovePendingSpecializedNodeInvite {
                    request: RemovePendingSpecializedNodeInvite { nonce },
                })
                .await
                .expect("Mailbox not to be dropped");
        }

        let _ignored = result?;

        Ok(nonce)
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

    #[must_use]
    pub fn sync_client(&self) -> &SyncClient {
        &self.sync_client
    }

    pub async fn sync(
        &self,
        context_id: Option<&ContextId>,
        peer_id: Option<&PeerId>,
    ) -> eyre::Result<()> {
        self.sync_client.sync(context_id, peer_id).await
    }

    pub async fn sync_namespace(&self, namespace_id: [u8; 32]) -> eyre::Result<()> {
        self.sync_client.sync_namespace(namespace_id).await
    }

    pub async fn request_namespace_join(
        &self,
        namespace_id: [u8; 32],
        invitation_bytes: Vec<u8>,
        joiner_public_key: PublicKey,
    ) -> eyre::Result<JoinBundle> {
        self.sync_client
            .request_namespace_join(namespace_id, invitation_bytes, joiner_public_key)
            .await
    }
}
