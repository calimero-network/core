use std::num::NonZeroUsize;

use calimero_crypto::SharedKey;
use calimero_network_primitives::client::NetworkClient;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use futures_util::future;
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use rand::Rng;
use tracing::debug;

use crate::sync::{BatchDelta, BroadcastMessage};

/// Handles all broadcasting operations for state deltas
pub struct BroadcastingService {
    network_client: NetworkClient,
}

impl BroadcastingService {
    pub fn new(network_client: NetworkClient) -> Self {
        Self { network_client }
    }

    /// Broadcast a single state delta via gossipsub
    pub async fn broadcast_single(
        &self,
        context: &Context,
        sender: &PublicKey,
        sender_key: &PrivateKey,
        artifact: Vec<u8>,
        height: NonZeroUsize,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            %sender,
            root_hash=%context.root_hash,
            "Broadcasting single state delta"
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
            root_hash: context.root_hash,
            artifact: encrypted.into(),
            height,
            nonce,
        };

        let payload = borsh::to_vec(&payload)?;
        let topic = TopicHash::from_raw(context.id);

        let _ignored = self.network_client.publish(topic, payload).await?;

        Ok(())
    }

    /// Broadcast multiple state deltas in a single message
    pub async fn broadcast_batch(
        &self,
        context: &Context,
        sender: &PublicKey,
        sender_key: &PrivateKey,
        deltas: Vec<(Vec<u8>, NonZeroUsize)>,
    ) -> eyre::Result<()> {
        if deltas.is_empty() {
            return Ok(());
        }

        debug!(
            context_id=%context.id,
            %sender,
            root_hash=%context.root_hash,
            delta_count=deltas.len(),
            "Broadcasting batch state deltas"
        );

        if self.get_peers_count(Some(&context.id)).await == 0 {
            return Ok(());
        }

        let shared_key = SharedKey::from_sk(sender_key);
        let nonce = rand::thread_rng().gen();

        let mut batch_deltas = Vec::with_capacity(deltas.len());
        for (artifact, height) in deltas {
            let encrypted = shared_key
                .encrypt(artifact, nonce)
                .ok_or_eyre("failed to encrypt artifact")?;

            batch_deltas.push(BatchDelta {
                artifact: encrypted.into(),
                height,
            });
        }

        let payload = BroadcastMessage::BatchStateDelta {
            context_id: context.id,
            author_id: *sender,
            root_hash: context.root_hash,
            deltas: batch_deltas,
            nonce,
        };

        let payload = borsh::to_vec(&payload)?;
        let topic = TopicHash::from_raw(context.id);

        let _ignored = self.network_client.publish(topic, payload).await?;

        Ok(())
    }

    /// Get the number of peers for a context
    async fn get_peers_count(&self, context: Option<&ContextId>) -> usize {
        let Some(context) = context else {
            return self.network_client.peer_count().await;
        };

        let topic = TopicHash::from_raw(*context);
        self.network_client.mesh_peer_count(topic).await
    }
}
