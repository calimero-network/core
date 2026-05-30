#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use async_stream::stream;
use calimero_context_config::types::GovernancePosition;
use calimero_crypto::SharedKey;
use calimero_network_primitives::client::{is_no_peers_subscribed_error, NetworkClient};
use calimero_network_primitives::config::GOSSIPSUB_MESH_N_LOW;
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

/// Parameters for a direct open-subgroup join request (issue #2357).
/// Counterpart to [`NamespaceJoinParams`] for the inherited self-join
/// path: joiner asks a peer holding the subgroup key for it directly,
/// proving authority via their `MembershipPath::Inherited` membership
/// (validated by the responder against the local store).
#[derive(Debug)]
pub struct OpenSubgroupJoinParams {
    pub namespace_id: [u8; 32],
    pub subgroup_id: [u8; 32],
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
    open_subgroup_join_tx: mpsc::Sender<(
        OpenSubgroupJoinParams,
        oneshot::Sender<eyre::Result<Vec<u8>>>,
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
        open_subgroup_join_tx: mpsc::Sender<(
            OpenSubgroupJoinParams,
            oneshot::Sender<eyre::Result<Vec<u8>>>,
        )>,
    ) -> Self {
        Self {
            ctx_sync_tx,
            ns_sync_tx,
            ns_join_tx,
            open_subgroup_join_tx,
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

    /// Send a direct open-subgroup join request to a mesh peer and await
    /// the response. Returns the borsh-serialized `KeyEnvelope` bytes
    /// the joiner unwraps locally with their namespace-identity SK.
    /// The SyncManager handles peer selection + stream I/O.
    pub async fn request_open_subgroup_join(
        &self,
        namespace_id: [u8; 32],
        subgroup_id: [u8; 32],
        joiner_public_key: PublicKey,
    ) -> eyre::Result<Vec<u8>> {
        let (tx, rx) = oneshot::channel();
        let params = OpenSubgroupJoinParams {
            namespace_id,
            subgroup_id,
            joiner_public_key,
        };
        self.open_subgroup_join_tx
            .send((params, tx))
            .await
            .map_err(|_| eyre::eyre!("open subgroup join channel closed"))?;
        rx.await
            .map_err(|_| eyre::eyre!("open subgroup join response channel dropped"))?
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
/// Gossipsub `mesh_n_low`. Used by Phase-1 readiness in
/// `governance_broadcast::assert_transport_ready` as the upper bound for
/// `required = min(mesh_n_low, known_subscribers)`.
///
/// Source: `GOSSIPSUB_MESH_N_LOW` in `calimero_network_primitives::config`,
/// which is the same value passed to `gossipsub::ConfigBuilder::mesh_n_low`
/// in `crates/network/src/behaviour.rs`. A mismatch between the gate and
/// the actual gossipsub config would either reject healthy publishes
/// (gate too high — the mesh never reaches the required size) or admit
/// publishes on an unhealthy mesh (gate too low).
fn gossipsub_mesh_n_low_default() -> usize {
    GOSSIPSUB_MESH_N_LOW
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

    /// Notify the readiness FSM that a namespace governance op was just
    /// applied locally on the publisher path. The gossipsub-receive path
    /// notifies `ReadinessManager` directly (it has the actor address);
    /// the publisher path lives in `crates/context`, which has no line
    /// into the node-side actor system, so the signal hops via
    /// `NodeMessage::ForwardNamespaceOpApplied` and lands at the same
    /// `Handler<NamespaceOpApplied>` after the routing step.
    ///
    /// Best-effort: `try_send` queues into the `LazyRecipient` mailbox
    /// when the receiver is not yet wired (early startup) and otherwise
    /// returns `Err(SendError::Full)` only if the mailbox saturates,
    /// which would indicate a stalled NodeManager rather than a missing
    /// signal. Either way the local DAG is already advanced; the FSM
    /// will catch up on its next tick or when the next op fires.
    pub fn notify_namespace_op_applied(&self, namespace_id: [u8; 32]) {
        if let Err(err) = self
            .node_manager
            .try_send(NodeMessage::ForwardNamespaceOpApplied { namespace_id })
        {
            warn!(
                ?err,
                namespace_id = %hex::encode(namespace_id),
                "failed to enqueue NamespaceOpApplied signal — readiness FSM will \
                 lag for this namespace until the next op fires or a peer beacon arrives"
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
    ///
    /// # Why this path keeps the mesh-peer wait (unlike `broadcast`)
    ///
    /// `broadcast` / `broadcast_heartbeat` dropped their `mesh_peer_count == 0`
    /// gate and silence `PublishError::NoPeersSubscribedToTopic` because state
    /// deltas have a sync-pull recovery path (HashComparison via heartbeats):
    /// a drop into an empty topic is recoverable by the next pull. Governance
    /// ops carry no such recovery — a `ContextRegistered` lost on cold start
    /// cannot be re-derived by the receiver, only re-published. So the
    /// publisher-side wait stays: we poll for at least one mesh peer up to
    /// `MAX_WAIT` before publishing, log when we give up, and propagate any
    /// publish error (including `NoPeersSubscribedToTopic`) so the caller can
    /// decide whether to retry/re-announce.
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

    /// Publish raw payload on the namespace topic `ns/<hex(namespace_id)>`
    /// immediately, without the "wait for mesh, then publish anyway" loop of
    /// [`publish_on_namespace`](Self::publish_on_namespace).
    ///
    /// Returns the mesh peer count observed at publish time so a caller running
    /// its own re-announce loop (e.g. the fleet-join admission wait) can tell a
    /// publish into a live mesh apart from a publish into an empty mesh — the
    /// latter is lost forever because gossipsub does not replay. Re-announcing
    /// each poll cycle means a *later* mesh window still receives a fresh copy,
    /// which a single up-front publish (the bug this fixes) never could.
    ///
    /// Kept separate from [`publish_on_namespace`](Self::publish_on_namespace)
    /// so the up-front wait-then-publish semantics other callers rely on are
    /// untouched; this is the opt-in, per-cycle building block.
    ///
    /// Like [`publish_on_namespace`](Self::publish_on_namespace), this method
    /// propagates `PublishError::NoPeersSubscribedToTopic` to the caller
    /// rather than silencing it — that's the exact "publish into an empty
    /// mesh" signal the re-announce loop needs to know it must keep trying.
    /// State-delta paths (`broadcast` / `broadcast_heartbeat`) silence the
    /// same error because they have sync-pull recovery; governance does not.
    pub async fn publish_on_namespace_now(
        &self,
        namespace_id: [u8; 32],
        payload: Vec<u8>,
    ) -> eyre::Result<usize> {
        let topic_str = format!("ns/{}", hex::encode(namespace_id));
        let topic = TopicHash::from_raw(topic_str);

        let mesh_peers = self.network_client.mesh_peer_count(topic.clone()).await;
        let _ignored = self.network_client.publish(topic, payload).await?;
        Ok(mesh_peers)
    }

    pub async fn get_peers_count(&self, context: Option<&ContextId>) -> usize {
        let Some(context) = context else {
            return self.network_client.peer_count().await;
        };

        let topic = TopicHash::from_raw(*context);

        self.network_client.mesh_peer_count(topic).await
    }

    /// Snapshot of the local node's libp2p connectivity state — relays,
    /// rendezvous registrations, DCUtR upgrade outcomes, AutoNAT v2
    /// reachability. Backs `GET /admin-api/network/status`.
    pub async fn network_status(
        &self,
    ) -> calimero_network_primitives::network_status::NetworkStatusSnapshot {
        self.network_client.network_status().await
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
        governance_position: Option<GovernancePosition>,
        key_id: [u8; 32],
        delta_signature: Option<[u8; 64]>,
        // `GroupMeta.app_key` the sender is executing under. `None` for
        // non-group contexts or when the meta row could not be resolved.
        producing_app_key: Option<[u8; 32]>,
    ) -> eyre::Result<()> {
        info!(
            context_id=%context.id,
            %sender,
            root_hash=%context.root_hash,
            delta_id=?delta_id,
            parent_count=parent_ids.len(),
            governance_dag_heads_len = governance_position
                .as_ref()
                .map(|p| p.governance_dag_heads.len())
                .unwrap_or(0),
            "Sending state delta"
        );

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
            governance_position,
            key_id,
            delta_signature,
            producing_app_key,
        };

        let payload = borsh::to_vec(&payload)?;

        let topic = TopicHash::from_raw(context.id);

        // Previously this returned early when `mesh_peer_count == 0`,
        // which silently dropped state deltas during the cold-start
        // window where peers are subscribed-but-not-yet-mesh. With
        // `flood_publish(true)` (#2352), gossipsub itself decides
        // reachability, so the application-level gate was both wrong
        // (missed flood-eligible peers) and silent (no logs). Attempt
        // the publish unconditionally and let gossipsub report the
        // actual outcome; convert the one cold-start error variant
        // to a debug log because the receiver's sync-pull path
        // (HashComparison via heartbeats) recovers any drop.
        match self.network_client.publish(topic, payload).await {
            Ok(_) => Ok(()),
            Err(err) if is_no_peers_subscribed_error(&err) => {
                debug!(
                    context_id = %context.id,
                    delta_id = ?delta_id,
                    "no peers subscribed to context topic, state delta dropped (sync-pull will recover)"
                );
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    pub async fn broadcast_heartbeat(
        &self,
        context_id: &ContextId,
        root_hash: calimero_primitives::hash::Hash,
        dag_heads: Vec<[u8; 32]>,
    ) -> eyre::Result<()> {
        let payload = BroadcastMessage::HashHeartbeat {
            context_id: *context_id,
            root_hash,
            dag_heads,
        };

        let payload = borsh::to_vec(&payload)?;
        let topic = TopicHash::from_raw(*context_id);

        // See `broadcast`: drop the application-level zero-peers gate so
        // gossipsub's `flood_publish` decides reachability, and silence
        // the cold-start `NoPeersSubscribed` variant — the next
        // heartbeat tick (~5 s) carries fresh state regardless.
        match self.network_client.publish(topic, payload).await {
            Ok(_) => Ok(()),
            Err(err) if is_no_peers_subscribed_error(&err) => {
                debug!(
                    %context_id,
                    "no peers subscribed to context topic, heartbeat dropped (next tick will carry fresh state)"
                );
                Ok(())
            }
            Err(err) => Err(err),
        }
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

    pub async fn request_open_subgroup_join(
        &self,
        namespace_id: [u8; 32],
        subgroup_id: [u8; 32],
        joiner_public_key: PublicKey,
    ) -> eyre::Result<Vec<u8>> {
        self.sync_client
            .request_open_subgroup_join(namespace_id, subgroup_id, joiner_public_key)
            .await
    }
}

#[cfg(test)]
mod publish_on_namespace_now_tests {
    //! Unit tests for the re-announce building block
    //! [`NodeClient::publish_on_namespace_now`] and the re-announce-until-
    //! admitted loop it powers in the fleet-join handler.
    //!
    //! The fleet-join admission wait lives in `calimero-server` (it needs the
    //! `ctx_client` admission read, which this crate does not see), so the loop
    //! itself is reproduced here against the same publish primitive. This is the
    //! smallest real unit: a live `NetworkClient` backed by a stub network actor
    //! that counts `Publish` messages and reports a settable mesh peer count —
    //! no libp2p transport, no server crate. The full owner-side admission path
    //! is covered by `calimero-node`'s `local_governance_node_e2e.rs`.
    //!
    //! Runs under `#[actix::test]` (single-threaded actix System) so the stub
    //! actor's mailbox is pumped by the same runtime that drives the client's
    //! `.await`s — `Actor::create` + `LazyRecipient::init`, the documented
    //! pattern from `calimero-utils-actix`'s own `lazy_tests.rs`.

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use actix::Actor;
    use calimero_blobstore::config::BlobStoreConfig;
    use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
    use calimero_network_primitives::client::NetworkClient;
    use calimero_network_primitives::messages::{MessageId, NetworkMessage};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tokio::sync::{broadcast, mpsc};

    use super::{BlobManager, NodeClient, SyncClient};

    /// Stub network actor: records how many times a `Publish` is requested and
    /// reports whatever mesh peer count the test sets via the shared atomic.
    /// Resolves `Publish`/`MeshPeerCount` outcomes so the awaiting client future
    /// completes; every other variant is dropped (none are reached here).
    struct CountingNetworkActor {
        publish_count: Arc<AtomicUsize>,
        mesh_peers: Arc<AtomicUsize>,
    }

    impl Actor for CountingNetworkActor {
        type Context = actix::Context<Self>;
    }

    impl actix::Handler<NetworkMessage> for CountingNetworkActor {
        type Result = ();

        fn handle(&mut self, msg: NetworkMessage, _ctx: &mut Self::Context) -> Self::Result {
            match msg {
                NetworkMessage::MeshPeerCount { outcome, .. } => {
                    let _ = outcome.send(self.mesh_peers.load(Ordering::SeqCst));
                }
                NetworkMessage::Publish { outcome, .. } => {
                    let _prev = self.publish_count.fetch_add(1, Ordering::SeqCst);
                    let _ = outcome.send(Ok(MessageId(b"stub".to_vec())));
                }
                _ => {}
            }
        }
    }

    /// Build a `NodeClient` whose `network_client` is wired to a freshly started
    /// [`CountingNetworkActor`] on the current actix System. Only the network
    /// path is exercised by `publish_on_namespace_now`; the remaining fields are
    /// minimal real stubs. Returns the client plus the shared publish-count and
    /// mesh-peer atomics for assertions. The `TempDir` is returned so the
    /// caller keeps the blobstore filesystem alive for the test's duration.
    async fn make_client() -> (
        NodeClient,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        tempfile::TempDir,
    ) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = Store::new(Arc::new(InMemoryDB::owned()));

        let blob_cfg =
            BlobStoreConfig::new(tmp.path().to_path_buf().try_into().expect("utf8 blob path"));
        let fs = FileSystem::new(&blob_cfg).await.expect("blob fs");
        let blob_manager = BlobManager::new(BlobStore::new(store.clone(), fs));

        let network_recipient = LazyRecipient::<NetworkMessage>::new();
        let network_client = NetworkClient::new(network_recipient.clone());

        let publish_count = Arc::new(AtomicUsize::new(0));
        let mesh_peers = Arc::new(AtomicUsize::new(0));

        let actor = CountingNetworkActor {
            publish_count: Arc::clone(&publish_count),
            mesh_peers: Arc::clone(&mesh_peers),
        };
        let _addr = CountingNetworkActor::create(move |ctx| {
            assert!(network_recipient.init(ctx), "network recipient init");
            actor
        });

        let (event_sender, _) = broadcast::channel(16);
        let (ctx_sync_tx, _ctx_sync_rx) = mpsc::channel(8);
        let (ns_sync_tx, _ns_sync_rx) = mpsc::channel(8);
        let (ns_join_tx, _ns_join_rx) = mpsc::channel(8);
        let (open_subgroup_join_tx, _open_rx) = mpsc::channel(8);
        let sync_client =
            SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

        let node_client = NodeClient::new(
            store,
            blob_manager,
            network_client,
            LazyRecipient::new(),
            event_sender,
            sync_client,
            String::new(),
            None,
        );

        (node_client, publish_count, mesh_peers, tmp)
    }

    /// `publish_on_namespace_now` publishes exactly once per call and reports the
    /// mesh peer count observed at publish time (here: 0 — the empty-mesh case
    /// that silently dropped the one-shot announce before this fix).
    #[actix::test]
    async fn publishes_once_and_reports_empty_mesh() {
        let (client, publish_count, _mesh, _tmp) = make_client().await;

        let observed = client
            .publish_on_namespace_now([0x11; 32], b"announce".to_vec())
            .await
            .expect("publish_on_namespace_now");

        assert_eq!(observed, 0, "no mesh peers were set");
        assert_eq!(
            publish_count.load(Ordering::SeqCst),
            1,
            "exactly one publish per call"
        );
    }

    /// `publish_on_namespace_now` surfaces a non-zero mesh peer count when one
    /// is present — the signal a caller uses to know the announce landed in a
    /// live mesh rather than an empty one.
    #[actix::test]
    async fn reports_live_mesh_peer_count() {
        let (client, _publish_count, mesh, _tmp) = make_client().await;
        mesh.store(2, Ordering::SeqCst);

        let observed = client
            .publish_on_namespace_now([0x33; 32], b"announce".to_vec())
            .await
            .expect("publish_on_namespace_now");

        assert_eq!(observed, 2, "must report the live mesh peer count");
    }

    /// Locks in the P1 fix: a re-announce loop that publishes every cycle while
    /// not admitted publishes MORE THAN ONCE over the wait window (the one-shot
    /// bug published exactly once), and STOPS the moment admission is observed —
    /// no further announces after admitted. This mirrors the integrated loop in
    /// `crates/server/src/admin/handlers/tee/fleet_join.rs`.
    #[actix::test]
    async fn reannounce_loop_publishes_more_than_once_then_stops_on_admission() {
        let (client, publish_count, _mesh, _tmp) = make_client().await;

        // Mirror the fleet-join handler loop: publish up front, then on each
        // not-yet-admitted cycle re-check admission, sleep, then re-publish
        // (re-publish AFTER the sleep, as the handler does, so the first
        // re-announce doesn't fire back-to-back with the up-front publish).
        // Admission flips true after a few cycles; once true the loop must break
        // BEFORE publishing again. A fast poll keeps the test sub-second.
        const ADMIT_AFTER_CYCLES: usize = 3;
        const POLL: Duration = Duration::from_millis(10);
        const MAX_CYCLES: usize = 50; // hard safety bound

        // First (up-front) announce, as the handler does before its loop.
        let _ = client
            .publish_on_namespace_now([0x22; 32], b"announce".to_vec())
            .await
            .expect("first announce");

        let mut cycles = 0;
        let mut admitted = false;
        while cycles < MAX_CYCLES {
            // Admission check FIRST so we never re-announce after admitted.
            if cycles >= ADMIT_AFTER_CYCLES {
                admitted = true;
                break;
            }
            tokio::time::sleep(POLL).await;
            let _ = client
                .publish_on_namespace_now([0x22; 32], b"announce".to_vec())
                .await
                .expect("re-announce");
            cycles += 1;
        }

        assert!(admitted, "loop must observe admission");
        let total = publish_count.load(Ordering::SeqCst);
        assert!(
            total > 1,
            "re-announce must publish more than once over the wait window, got {total}"
        );
        // up-front (1) + one per not-yet-admitted cycle (ADMIT_AFTER_CYCLES).
        assert_eq!(
            total,
            1 + ADMIT_AFTER_CYCLES,
            "must stop announcing the instant admission is observed"
        );
    }
}
