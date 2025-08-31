use std::num::NonZeroUsize;

use calimero_crypto::SharedKey;
use calimero_network_primitives::client::NetworkClient;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use eyre::OptionExt;
use libp2p::gossipsub::TopicHash;
use rand::Rng;
use tracing::debug;

use crate::sync::{BatchDelta, BroadcastMessage};

/// Handles all broadcasting operations for state deltas
#[derive(Debug)]
pub struct BroadcastingService {
    network_client: NetworkClient,
}

impl BroadcastingService {
    pub fn new(network_client: NetworkClient) -> Self {
        Self { network_client }
    }

    /// Check if batch processing is available and beneficial
    pub async fn should_use_batch_processing(
        &self,
        context_id: &ContextId,
        pending_deltas: &[(Vec<u8>, NonZeroUsize)],
    ) -> bool {
        // Use batch processing if we have multiple deltas and peers are available
        let peer_count = self.get_peers_count(Some(context_id)).await;
        let has_multiple_deltas = pending_deltas.len() > 1;
        let has_peers = peer_count > 0;
        
        debug!(
            context_id=%context_id,
            peer_count,
            delta_count=pending_deltas.len(),
            should_batch=has_multiple_deltas && has_peers,
            "Batch processing decision"
        );
        
        has_multiple_deltas && has_peers
    }

    /// Check if direct P2P communication is available
    pub async fn should_use_direct_p2p(
        &self,
        context_id: &ContextId,
        target_peer: Option<&libp2p::PeerId>,
    ) -> bool {
        // For now, always use gossipsub as fallback
        // Direct P2P would require maintaining a list of trusted peers
        // and checking if the target peer is available for direct communication
        
        debug!(
            context_id=%context_id,
            target_peer=?target_peer,
            "Direct P2P decision: using gossipsub fallback"
        );
        
        false // Always use gossipsub for now
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
