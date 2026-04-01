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
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;

use crate::messages::{
    NodeMessage, RegisterPendingSpecializedNodeInvite, RemovePendingSpecializedNodeInvite,
};
use crate::sync::{BroadcastMessage, MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES};

mod alias;
mod application;
mod blob;

#[derive(Clone, Debug)]
pub struct NodeClient {
    datastore: Store,
    blobstore: BlobManager,
    network_client: NetworkClient,
    node_manager: LazyRecipient<NodeMessage>,
    event_sender: broadcast::Sender<NodeEvent>,
    ctx_sync_tx: mpsc::Sender<(Option<ContextId>, Option<PeerId>)>,
    specialized_node_invite_topic: String,
}

impl NodeClient {
    #[must_use]
    pub fn new(
        datastore: Store,
        blobstore: BlobManager,
        network_client: NetworkClient,
        node_manager: LazyRecipient<NodeMessage>,
        event_sender: broadcast::Sender<NodeEvent>,
        ctx_sync_tx: mpsc::Sender<(Option<ContextId>, Option<PeerId>)>,
        specialized_node_invite_topic: String,
    ) -> Self {
        Self {
            datastore,
            blobstore,
            network_client,
            node_manager,
            event_sender,
            ctx_sync_tx,
            specialized_node_invite_topic,
        }
    }

    pub async fn subscribe(&self, context_id: &ContextId) -> eyre::Result<()> {
        let topic = IdentTopic::new(context_id);

        let _ignored = self.network_client.subscribe(topic).await?;

        info!(%context_id, "Subscribed to context");

        Ok(())
    }

    pub async fn unsubscribe(&self, context_id: &ContextId) -> eyre::Result<()> {
        let topic = IdentTopic::new(context_id);

        let _ignored = self.network_client.unsubscribe(topic).await?;

        info!(%context_id, "Unsubscribed from context");

        Ok(())
    }

    pub async fn subscribe_group(&self, group_id: [u8; 32]) -> eyre::Result<()> {
        let topic = IdentTopic::new(format!("group/{}", hex::encode(group_id)));
        let _ignored = self.network_client.subscribe(topic).await?;
        info!(?group_id, "Subscribed to group topic");
        Ok(())
    }

    pub async fn unsubscribe_group(&self, group_id: [u8; 32]) -> eyre::Result<()> {
        let topic = IdentTopic::new(format!("group/{}", hex::encode(group_id)));
        let _ignored = self.network_client.unsubscribe(topic).await?;
        info!(?group_id, "Unsubscribed from group topic");
        Ok(())
    }

    pub async fn publish_on_group(&self, group_id: [u8; 32], payload: Vec<u8>) -> eyre::Result<()> {
        let topic_str = format!("group/{}", hex::encode(group_id));
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
                    ?group_id,
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

    pub async fn broadcast_group_mutation(
        &self,
        group_id: [u8; 32],
        mutation_kind: crate::sync::GroupMutationKind,
    ) -> eyre::Result<()> {
        let topic_str = format!("group/{}", hex::encode(group_id));
        let topic = TopicHash::from_raw(topic_str);

        let peers = self.network_client.mesh_peer_count(topic.clone()).await;
        if peers == 0 {
            debug!(
                ?mutation_kind,
                "no peers on group topic, skipping broadcast"
            );
            return Ok(());
        }

        let payload = BroadcastMessage::GroupMutationNotification {
            group_id,
            mutation_kind,
        };
        let payload_bytes = borsh::to_vec(&payload)?;

        if let Err(err) = self.network_client.publish(topic, payload_bytes).await {
            warn!(?group_id, %err, "failed to publish group mutation notification");
        }

        Ok(())
    }

    /// Publish a borsh-encoded `SignedGroupOp` (`calimero_context_primitives::local_governance`)
    /// on the group gossip topic `group/<hex(group_id)>`.
    ///
    /// Enforces [`MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES`] on `signed_op_borsh`.
    ///
    /// If there are no mesh peers on the group topic, the publish is skipped and a **warn** is
    /// logged (silent skips make ops easy to miss in production).
    pub async fn publish_signed_group_op(
        &self,
        group_id: [u8; 32],
        delta_id: [u8; 32],
        parent_ids: Vec<[u8; 32]>,
        signed_op_borsh: Vec<u8>,
    ) -> eyre::Result<()> {
        if signed_op_borsh.len() > MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES {
            eyre::bail!(
                "signed group op payload exceeds max ({} > {})",
                signed_op_borsh.len(),
                MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES
            );
        }

        let topic_str = format!("group/{}", hex::encode(group_id));
        let topic = TopicHash::from_raw(topic_str);

        let peers = self.network_client.mesh_peer_count(topic.clone()).await;
        if peers == 0 {
            warn!(
                ?group_id,
                "no peers on group topic, skipping signed group op broadcast"
            );
            return Ok(());
        }

        let payload = BroadcastMessage::GroupGovernanceDelta {
            group_id,
            delta_id,
            parent_ids,
            payload: signed_op_borsh,
        };
        let payload_bytes = borsh::to_vec(&payload)?;

        if let Err(err) = self.network_client.publish(topic, payload_bytes).await {
            warn!(?group_id, %err, "failed to publish signed group op");
        }

        Ok(())
    }

    pub async fn publish_group_heartbeat(
        &self,
        group_id: [u8; 32],
        dag_heads: Vec<[u8; 32]>,
        member_count: u32,
    ) -> eyre::Result<()> {
        let topic_str = format!("group/{}", hex::encode(group_id));
        let topic = TopicHash::from_raw(topic_str);

        let payload = BroadcastMessage::GroupStateHeartbeat {
            group_id,
            dag_heads,
            member_count,
        };
        let payload_bytes = borsh::to_vec(&payload)?;
        if let Err(err) = self.network_client.publish(topic, payload_bytes).await {
            debug!(?group_id, %err, "failed to publish group heartbeat");
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
}
