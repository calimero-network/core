//! Delta request protocol for DAG gap filling
//!
//! When a node receives a delta with missing parents, it uses this protocol
//! to request the missing deltas from peers.

use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_storage::delta::CausalDelta;
use eyre::{bail, OptionExt, Result};
use tracing::{debug, info, warn};

use super::manager::SyncManager;
use super::tracking::Sequencer;

impl SyncManager {
    /// Request multiple missing deltas from a peer and add them to the DAG
    pub async fn request_missing_deltas(
        &self,
        context_id: ContextId,
        missing_ids: Vec<[u8; 32]>,
        source: libp2p::PeerId,
        delta_store: crate::delta_store::DeltaStore,
    ) -> Result<()> {
        info!(
            %context_id,
            ?source,
            missing_count = missing_ids.len(),
            "Requesting missing parent deltas from peer"
        );

        // Open stream to peer
        let mut stream = self.network_client.open_stream(source).await?;

        // Request each missing delta
        for missing_id in missing_ids {
            match self
                .request_delta(&context_id, missing_id, &mut stream)
                .await
            {
                Ok(Some(parent_delta)) => {
                    info!(
                        %context_id,
                        delta_id = ?missing_id,
                        action_count = parent_delta.actions.len(),
                        "Received missing parent delta, adding to DAG"
                    );

                    // Convert to DAG delta format
                    let dag_delta = calimero_dag::CausalDelta {
                        id: parent_delta.id,
                        parents: parent_delta.parents,
                        payload: parent_delta.actions,
                        hlc: parent_delta.hlc,
                        expected_root_hash: parent_delta.expected_root_hash,
                    };

                    if let Err(e) = delta_store.add_delta(dag_delta).await {
                        warn!(?e, %context_id, delta_id = ?missing_id, "Failed to add requested delta to DAG");
                    }
                }
                Ok(None) => {
                    warn!(%context_id, delta_id = ?missing_id, "Peer doesn't have requested delta");
                }
                Err(e) => {
                    warn!(?e, %context_id, delta_id = ?missing_id, "Failed to request delta");
                    break; // Stop requesting if stream fails
                }
            }
        }

        Ok(())
    }

    /// Request a specific delta from a peer
    pub(crate) async fn request_delta(
        &self,
        context_id: &ContextId,
        delta_id: [u8; 32],
        stream: &mut Stream,
    ) -> Result<Option<CausalDelta>> {
        info!(
            %context_id,
            delta_id = ?delta_id,
            "Requesting missing delta from peer"
        );

        // Send request
        let msg = StreamMessage::Init {
            context_id: *context_id,
            party_id: [0; 32].into(), // TODO: Get actual identity
            payload: InitPayload::DeltaRequest {
                context_id: *context_id,
                delta_id,
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;

        // Wait for response
        let timeout_budget = self.sync_config.timeout;

        match super::stream::recv(stream, None, timeout_budget).await? {
            Some(StreamMessage::Message {
                payload: MessagePayload::DeltaResponse { delta },
                ..
            }) => {
                // Deserialize delta
                let causal_delta: CausalDelta = borsh::from_slice(&delta)?;

                // Verify delta ID matches
                if causal_delta.id != delta_id {
                    bail!(
                        "Received delta ID mismatch: requested {:?}, got {:?}",
                        delta_id,
                        causal_delta.id
                    );
                }

                info!(
                    %context_id,
                    delta_id = ?delta_id,
                    action_count = causal_delta.actions.len(),
                    "Received requested delta"
                );

                Ok(Some(causal_delta))
            }
            Some(StreamMessage::Message {
                payload: MessagePayload::DeltaNotFound,
                ..
            }) => {
                debug!(
                    %context_id,
                    delta_id = ?delta_id,
                    "Peer doesn't have requested delta"
                );
                Ok(None)
            }
            Some(StreamMessage::OpaqueError) => {
                bail!("Peer encountered error processing delta request");
            }
            other => {
                bail!("Unexpected response to delta request: {:?}", other);
            }
        }
    }

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
