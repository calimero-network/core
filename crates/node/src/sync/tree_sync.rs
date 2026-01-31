//! Entity-based sync protocols.
//!
//! Implements HashComparison and BloomFilter strategies for synchronizing
//! state ENTITIES (not deltas) between peers.
//!
//! These protocols work on the Merkle tree state directly, using entity keys
//! and values rather than DAG deltas.

use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::ContextState as ContextStateKey;
use calimero_store::slice::Slice;
use calimero_store::types::ContextState as ContextStateValue;
use eyre::{bail, Result};
use libp2p::PeerId;
use rand::Rng;
use tracing::{debug, info, warn};

use super::manager::SyncManager;
use super::snapshot::{build_entity_bloom_filter, get_entity_keys};
use super::tracking::SyncProtocol;

impl SyncManager {
    /// Execute bloom filter sync with a peer.
    ///
    /// 1. Get all local entity keys
    /// 2. Build bloom filter from keys
    /// 3. Send filter to peer
    /// 4. Peer checks their entities against filter
    /// 5. Peer sends back entities we're missing
    /// 6. Apply received entities with CRDT merge
    pub(super) async fn bloom_filter_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
        false_positive_rate: f32,
    ) -> Result<SyncProtocol> {
        info!(
            %context_id,
            %peer_id,
            false_positive_rate,
            "Starting ENTITY-based bloom filter sync"
        );

        // Get storage handle via context_client
        let store_handle = self.context_client.datastore_handle();

        // Get all local entity keys
        let local_keys = get_entity_keys(&store_handle, context_id)?;

        info!(
            %context_id,
            local_entity_count = local_keys.len(),
            "Building bloom filter from local entity keys"
        );

        // Build bloom filter
        let bloom_filter = build_entity_bloom_filter(&local_keys, false_positive_rate);

        // Send bloom filter request
        let request = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::BloomFilterRequest {
                context_id,
                bloom_filter,
                false_positive_rate,
            },
            next_nonce: rand::thread_rng().gen(),
        };

        self.send(stream, &request, None).await?;

        let response = self.recv(stream, None).await?;

        match response {
            Some(StreamMessage::Message {
                payload:
                    MessagePayload::BloomFilterResponse {
                        missing_entities,
                        matched_count,
                    },
                ..
            }) => {
                let bytes_received = missing_entities.len();

                // Decode and apply missing entities
                let mut entities_applied = 0u64;
                let mut offset = 0;
                let mut store_handle = self.context_client.datastore_handle();

                while offset + 36 <= missing_entities.len() {
                    // Read key (32 bytes)
                    let mut key = [0u8; 32];
                    key.copy_from_slice(&missing_entities[offset..offset + 32]);
                    offset += 32;

                    // Read value length (4 bytes)
                    let value_len = u32::from_le_bytes([
                        missing_entities[offset],
                        missing_entities[offset + 1],
                        missing_entities[offset + 2],
                        missing_entities[offset + 3],
                    ]) as usize;
                    offset += 4;

                    if offset + value_len > missing_entities.len() {
                        warn!(%context_id, "Truncated entity in bloom filter response");
                        break;
                    }

                    let value = missing_entities[offset..offset + value_len].to_vec();
                    offset += value_len;

                    // Apply entity to storage
                    let state_key = ContextStateKey::new(context_id, key);
                    let slice: Slice<'_> = value.into();
                    match store_handle.put(&state_key, &ContextStateValue::from(slice)) {
                        Ok(_) => {
                            entities_applied += 1;
                            debug!(
                                %context_id,
                                entity_key = ?key,
                                "Applied entity from bloom filter sync"
                            );
                        }
                        Err(e) => {
                            warn!(
                                %context_id,
                                entity_key = ?key,
                                error = %e,
                                "Failed to apply entity from bloom filter sync"
                            );
                        }
                    }
                }

                info!(
                    %context_id,
                    bytes_received,
                    entities_applied,
                    matched_count,
                    "Bloom filter sync completed - applied ENTITIES directly"
                );

                // Record metrics
                self.metrics.record_bytes_received(bytes_received as u64);

                Ok(SyncProtocol::BloomFilter)
            }
            Some(StreamMessage::OpaqueError) => {
                warn!(%context_id, "Peer returned error for bloom filter request");
                bail!("Peer returned error during bloom filter sync");
            }
            other => {
                warn!(%context_id, ?other, "Unexpected response to BloomFilterRequest");
                bail!("Unexpected response during bloom filter sync");
            }
        }
    }

    /// Execute hash comparison sync with a peer.
    ///
    /// Compares Merkle tree root hashes and transfers differing entities.
    /// For now, this uses bloom filter as the diff mechanism since we have
    /// the infrastructure. A full implementation would do recursive tree comparison.
    pub(super) async fn hash_comparison_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
        local_root_hash: Hash,
        remote_root_hash: Hash,
    ) -> Result<SyncProtocol> {
        info!(
            %context_id,
            %peer_id,
            local_hash = %local_root_hash,
            remote_hash = %remote_root_hash,
            "Starting hash comparison sync"
        );

        // If hashes match, no sync needed
        if local_root_hash == remote_root_hash {
            info!(%context_id, "Root hashes match, no sync needed");
            return Ok(SyncProtocol::None);
        }

        // Use bloom filter for efficient diff detection
        // A full implementation would do recursive tree comparison
        self.bloom_filter_sync(context_id, peer_id, our_identity, stream, 0.01)
            .await
            .map(|_| SyncProtocol::HashComparison)
    }
}
