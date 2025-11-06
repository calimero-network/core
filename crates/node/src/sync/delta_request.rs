//! Delta request protocol helpers.

use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_storage::delta::CausalDelta;
use eyre::{OptionExt, Result};
use tracing::{debug, info, warn};

use crate::NodeState;

use super::helpers::generate_nonce;
use super::stream::send;
use super::tracking::Sequencer;

/// Handle incoming delta request from a peer.
pub(crate) async fn handle_delta_request(
    context_client: &ContextClient,
    node_state: &NodeState,
    context_id: ContextId,
    delta_id: [u8; 32],
    stream: &mut Stream,
) -> Result<()> {
    info!(
        %context_id,
        delta_id = ?delta_id,
        "Handling delta request from peer"
    );

    use calimero_store::key;

    let handle = context_client.datastore_handle();
    let db_key = key::ContextDagDelta::new(context_id, delta_id);

    let response = if let Some(stored_delta) = handle.get(&db_key)? {
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
    } else if let Some(delta_store) = node_state.delta_stores.get(&context_id) {
        if let Some(dag_delta) = delta_store.get_delta(&delta_id).await {
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

    let mut sqx = Sequencer::default();
    let msg = StreamMessage::Message {
        sequence_id: sqx.next(),
        payload: response,
        next_nonce: generate_nonce(),
    };

    send(stream, &msg, None).await?;

    Ok(())
}

/// Handle incoming DAG heads request from a peer.
pub(crate) async fn handle_dag_heads_request(
    context_client: &ContextClient,
    context_id: ContextId,
    stream: &mut Stream,
) -> Result<()> {
    info!(
        %context_id,
        "Handling DAG heads request from peer"
    );

    let context = context_client
        .get_context(&context_id)?
        .ok_or_eyre("Context not found")?;

    info!(
        %context_id,
        heads_count = context.dag_heads.len(),
        root_hash = %context.root_hash,
        "Sending DAG heads to peer"
    );

    let mut sqx = Sequencer::default();
    let msg = StreamMessage::Message {
        sequence_id: sqx.next(),
        payload: MessagePayload::DagHeadsResponse {
            dag_heads: context.dag_heads,
            root_hash: context.root_hash,
        },
        next_nonce: generate_nonce(),
    };

    send(stream, &msg, None).await?;

    Ok(())
}
