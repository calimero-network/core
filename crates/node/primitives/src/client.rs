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
use tracing::info;

use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;

use crate::messages::{
    NodeMessage, RegisterPendingSpecializedNodeInvite, RemovePendingSpecializedNodeInvite,
};
use crate::sync::BroadcastMessage;

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
            // Sync hints are optional for backward compatibility
            sync_hints: None,
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
