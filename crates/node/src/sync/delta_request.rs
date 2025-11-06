//! Delta request protocol for DAG gap filling
//!
//! When a node receives a delta with missing parents, it uses this protocol
//! to request the missing deltas from peers.

use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_storage::delta::CausalDelta;
use eyre::{OptionExt, Result};
use tracing::{debug, info, warn};

use super::manager::SyncManager;
use super::tracking::Sequencer;

impl SyncManager {
    /// Handle incoming delta request from a peer
    pub async fn handle_delta_request(
        &self,
        context_id: ContextId,
        delta_id: [u8; 32],
        stream: &mut Stream,
    ) -> Result<()> {
        info!(
            %context_id,
            delta_id = ?delta_id,
            "Handling delta request from peer"
        );

        // Try RocksDB first (has full CausalDelta with HLC)
        use calimero_store::key;

        let handle = self.context_client.datastore_handle();
        let db_key = key::ContextDagDelta::new(context_id, delta_id);

        let response = if let Some(stored_delta) = handle.get(&db_key)? {
            // Found in RocksDB - reconstruct CausalDelta with HLC
            let actions: Vec<calimero_storage::interface::Action> =
                borsh::from_slice(&stored_delta.actions)?;

            let causal_delta = CausalDelta {
                id: stored_delta.delta_id,
                parents: stored_delta.parents,
                actions,
                hlc: stored_delta.hlc,
                expected_root_hash: stored_delta.expected_root_hash,
            };

            let serialized = borsh::to_vec(&causal_delta)?;

            debug!(
                %context_id,
                delta_id = ?delta_id,
                size = serialized.len(),
                source = "RocksDB",
                "Sending requested delta to peer"
            );

            MessagePayload::DeltaResponse {
                delta: serialized.into(),
            }
        } else if let Some(delta_store) = self.node_state.delta_stores.get(&context_id) {
            // Not in RocksDB yet (race condition after broadcast), try DeltaStore
            if let Some(dag_delta) = delta_store.get_delta(&delta_id).await {
                // dag::CausalDelta now includes HLC, so we can directly convert
                let causal_delta = CausalDelta {
                    id: dag_delta.id,
                    parents: dag_delta.parents,
                    actions: dag_delta.payload,
                    hlc: dag_delta.hlc,
                    expected_root_hash: dag_delta.expected_root_hash,
                };

                let serialized = borsh::to_vec(&causal_delta)?;

                debug!(
                    %context_id,
                    delta_id = ?delta_id,
                    size = serialized.len(),
                    source = "DeltaStore",
                    "Sending requested delta to peer"
                );

                MessagePayload::DeltaResponse {
                    delta: serialized.into(),
                }
            } else {
                warn!(
                    %context_id,
                    delta_id = ?delta_id,
                    "Requested delta not found in RocksDB or DeltaStore"
                );
                MessagePayload::DeltaNotFound
            }
        } else {
            warn!(
                %context_id,
                delta_id = ?delta_id,
                "Requested delta not found (no DeltaStore for context)"
            );
            MessagePayload::DeltaNotFound
        };

        // Send response
        let mut sqx = Sequencer::default();
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: response,
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;

        Ok(())
    }

    /// Handle incoming DAG heads request from a peer
    pub async fn handle_dag_heads_request(
        &self,
        context_id: ContextId,
        stream: &mut Stream,
        _nonce: Nonce,
    ) -> Result<()> {
        info!(
            %context_id,
            "Handling DAG heads request from peer"
        );

        // Get context to retrieve dag_heads and root_hash
        let context = self
            .context_client
            .get_context(&context_id)?
            .ok_or_eyre("Context not found")?;

        info!(
            %context_id,
            heads_count = context.dag_heads.len(),
            root_hash = %context.root_hash,
            "Sending DAG heads to peer"
        );

        // Send response
        let mut sqx = Sequencer::default();
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: MessagePayload::DagHeadsResponse {
                dag_heads: context.dag_heads,
                root_hash: context.root_hash,
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;

        Ok(())
    }
}
