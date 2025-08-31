#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use std::num::NonZeroUsize;

use async_stream::stream;
use calimero_blobstore::BlobManager;
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
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info};

use crate::broadcasting::BroadcastingService;
use crate::messages::NodeMessage;

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
}

impl NodeClient {
    pub fn new(
        datastore: Store,
        blobstore: BlobManager,
        network_client: NetworkClient,
        node_manager: LazyRecipient<NodeMessage>,
        event_sender: broadcast::Sender<NodeEvent>,
        ctx_sync_tx: mpsc::Sender<(Option<ContextId>, Option<PeerId>)>,
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

    /// Broadcast a single state delta
    pub async fn broadcast(
        &self,
        context: &Context,
        sender: &PublicKey,
        sender_key: &PrivateKey,
        artifact: Vec<u8>,
        height: NonZeroUsize,
    ) -> eyre::Result<()> {
        let broadcasting = BroadcastingService::new(self.network_client.clone());
        broadcasting
            .broadcast_single(context, sender, sender_key, artifact, height)
            .await
    }

    /// Broadcast multiple state deltas in a batch
    pub async fn broadcast_batch(
        &self,
        context: &Context,
        sender: &PublicKey,
        sender_key: &PrivateKey,
        deltas: Vec<(Vec<u8>, NonZeroUsize)>,
    ) -> eyre::Result<()> {
        let broadcasting = BroadcastingService::new(self.network_client.clone());
        broadcasting
            .broadcast_batch(context, sender, sender_key, deltas)
            .await
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
