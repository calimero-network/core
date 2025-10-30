//! State delta handling for BroadcastMessage::StateDelta
//!
//! **SRP**: This module has ONE job - process state deltas from peers using DAG

use calimero_context_primitives::client::ContextClient;
use calimero_crypto::Nonce;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, NodeEvent, StateMutationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt, Result};
use libp2p::PeerId;
use tracing::{debug, info, warn};

use crate::delta_store::DeltaStore;
use crate::utils::choose_stream;

/// Handles state delta received from a peer (DAG-based)
///
/// This processes incoming state mutations using a DAG structure.
/// No gap checking - deltas are applied when all parents are available.
///
/// # Flow
/// 1. Validates context exists
/// 2. Reconstructs CausalDelta from broadcast
/// 3. Adds to DeltaStore (applies if parents ready, pends otherwise)
/// 4. Requests missing parents if needed
/// 5. Executes event handlers
/// 6. Re-emits events to WebSocket clients
pub async fn handle_state_delta(
    node_clients: crate::NodeClients,
    node_state: crate::NodeState,
    network_client: calimero_network_primitives::client::NetworkClient,
    sync_timeout: std::time::Duration,
    source: PeerId,
    context_id: ContextId,
    author_id: PublicKey,
    delta_id: [u8; 32],
    parent_ids: Vec<[u8; 32]>,
    hlc: calimero_storage::logical_clock::HybridTimestamp,
    root_hash: Hash,
    artifact: Vec<u8>,
    nonce: Nonce,
    events: Option<Vec<u8>>,
) -> Result<()> {
    let Some(context) = node_clients.context.get_context(&context_id)? else {
        bail!("context '{}' not found", context_id);
    };

    info!(
        %context_id,
        %author_id,
        delta_id = ?delta_id,
        parent_count = parent_ids.len(),
        expected_root_hash = %root_hash,
        current_root_hash = %context.root_hash,
        "Received state delta"
    );

    // Get author's sender key to decrypt artifact
    let author_identity = node_clients
        .context
        .get_identity(&context_id, &author_id)?
        .ok_or_eyre("author identity not found")?;

    let sender_key = author_identity
        .sender_key
        .ok_or_eyre("author has no sender key")?;

    // Decrypt artifact
    let shared_key = calimero_crypto::SharedKey::from_sk(&sender_key.into());
    let decrypted_artifact = shared_key
        .decrypt(artifact, nonce)
        .ok_or_eyre("failed to decrypt artifact")?;

    // Deserialize decrypted artifact
    let storage_delta: calimero_storage::delta::StorageDelta =
        borsh::from_slice(&decrypted_artifact)?;

    let actions = match storage_delta {
        calimero_storage::delta::StorageDelta::Actions(actions) => actions,
        _ => bail!("Expected Actions variant in state delta"),
    };

    // Create delta using calimero-dag types (with Vec<Action> payload)
    let delta = calimero_dag::CausalDelta {
        id: delta_id,
        parents: parent_ids,
        payload: actions, // Note: renamed from 'actions' to 'payload'
        hlc,
        expected_root_hash: *root_hash,
    };

    // Get our identity for applying deltas
    let identities = node_clients
        .context
        .get_context_members(&context_id, Some(true));
    let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
        .await
        .transpose()?
    else {
        bail!("no owned identities found for context: {}", context_id);
    };

    // Get or create DeltaStore for this context
    let is_uninitialized = *context.root_hash == [0; 32];

    let delta_store = node_state
        .delta_stores
        .entry(context_id)
        .or_insert_with(|| {
            // Initialize with root (zero hash for now)
            DeltaStore::new(
                [0u8; 32],
                node_clients.context.clone(),
                context_id,
                our_identity,
            )
        });

    // Convert to owned DeltaStore for async operations
    let delta_store_ref = delta_store.clone();
    drop(delta_store);

    // Add delta (applies immediately if parents available, pends otherwise)
    let applied = delta_store_ref.add_delta(delta).await?;

    if !applied {
        // Delta is pending - request missing parents
        let missing = delta_store_ref.get_missing_parents().await;

        if !missing.is_empty() {
            warn!(
                %context_id,
                missing_count = missing.len(),
                context_is_uninitialized = is_uninitialized,
                has_events = events.is_some(),
                "Delta pending due to missing parents - requesting them from peer"
            );

            // Request missing deltas (blocking this handler until complete)
            if let Err(e) = request_missing_deltas(
                network_client,
                sync_timeout,
                context_id,
                missing,
                source,
                our_identity,
                delta_store_ref.clone(),
            )
            .await
            {
                warn!(?e, %context_id, ?source, "Failed to request missing deltas");
            }
        }
    }

    // Deserialize events ONCE if present (optimization: avoid double parse)
    let events_payload = if let Some(ref events_data) = events {
        match serde_json::from_slice::<Vec<ExecutionEvent>>(events_data) {
            Ok(payload) => Some(payload),
            Err(e) => {
                warn!(
                    %context_id,
                    error = %e,
                    "Failed to deserialize events, skipping handler execution and WebSocket emission"
                );
                None
            }
        }
    } else {
        None
    };

    // Execute event handlers ONLY if the delta was actually applied
    // NOTE: Handlers are NEVER executed on the author node that produced the events.
    // They are only executed on receiving nodes to avoid infinite loops and ensure
    // proper distributed execution (as per crates/context/src/handlers/execute.rs:279-281)
    //
    // CRITICAL: We must check if the delta was applied. If it's pending, handlers
    // will be lost because when the delta is applied later (via __calimero_sync_next),
    // the events data won't be available!
    if applied {
        if let Some(ref payload) = events_payload {
            if author_id != our_identity {
                info!(
                    %context_id,
                    %author_id,
                    %our_identity,
                    "Executing event handlers (delta applied, we are a receiving node)"
                );
                execute_event_handlers_parsed(
                    &node_clients.context,
                    &context_id,
                    &our_identity,
                    payload,
                )
                .await?;
            } else {
                info!(
                    %context_id,
                    %author_id,
                    "Skipping event handler execution (we are the author node)"
                );
            }
        }
    } else if events_payload.is_some() {
        warn!(
            %context_id,
            delta_id = ?delta_id,
            "Delta with events buffered as pending - handlers will NOT execute when delta is applied later!"
        );
    }

    // Emit state mutation to WebSocket clients (frontends)
    // Use already-parsed events (no re-deserialization!)
    if let Some(payload) = events_payload {
        emit_state_mutation_event_parsed(&node_clients.node, &context_id, root_hash, payload)?;
    }

    Ok(())
}

/// Execute event handlers for received events (from already-parsed payload)
///
/// # Handler Execution Model
///
/// **IMPORTANTMenuHandlers currently execute **sequentially** in the order they appear
/// in the events array. Future optimization may execute handlers in **parallel**.
///
/// ## Requirements for Application Handlers
///
/// Event handlers **MUST** satisfy these properties to be correct:
///
/// 1. **CommutativeMenuHandler order must not affect final state
///    - ✅ SAFE: CRDT operations (Counter::increment, UnorderedMap::insert)
///    - ❌ UNSAFE: Dependent operations (create → update → delete chains)
///
/// 2. **Independent**: Handlers must not share mutable state
///    - ✅ SAFE: Each handler modifies different CRDT keys
///    - ❌ UNSAFE: Multiple handlers modifying same entity
///
/// 3. **Idempotent**: Re-execution must be safe
///    - ✅ SAFE: CRDT operations (naturally idempotent)
///    - ❌ UNSAFE: External API calls (charge_payment, send_email)
///
/// 4. **No side effectsMenuHandlers should only modify CRDT state
///    - ✅ SAFE: Pure state updates
///    - ❌ UNSAFE: HTTP requests, file I/O, blockchain transactions
///
/// ## Current Handler Implementations (Audited 2025-10-27)
///
/// All handlers in the codebase are **CRDT-only** operations:
/// - `kv-store-with-handlers`: All handlers just call `Counter::increment()`
/// - Other apps: No handlers defined
///
/// **VerdictMenuCurrent handlers are **100% safe** for parallel execution.
///
/// ## Future Developers
///
/// If you're adding handlers that violate these assumptions:
/// 1. Document why parallelization is unsafe
/// 2. Consider refactoring to use CRDTs
/// 3. Or disable parallelization if absolutely necessary
async fn execute_event_handlers_parsed(
    context_client: &ContextClient,
    context_id: &ContextId,
    our_identity: &PublicKey,
    events_payload: &[ExecutionEvent],
) -> Result<()> {
    for event in events_payload {
        if let Some(handler_name) = &event.handler {
            debug!(
                %context_id,
                event_kind = %event.kind,
                handler_name = %handler_name,
                "Executing handler for event"
            );

            match context_client
                .execute(
                    context_id,
                    our_identity,
                    handler_name.clone(),
                    event.data.clone(),
                    vec![],
                    None,
                )
                .await
            {
                Ok(_handler_response) => {
                    debug!(
                        handler_name = %handler_name,
                        "Handler executed successfully"
                    );
                }
                Err(err) => {
                    warn!(
                        handler_name = %handler_name,
                        error = %err,
                        "Handler execution failed"
                    );
                }
            }
        }
    }

    Ok(())
}

/// Emit state mutation event to WebSocket clients (frontends)
///
/// Note: This is separate from node-to-node DAG synchronization.
/// - DAG broadcast (BroadcastMessage::StateDelta) = node-to-node sync
/// - WebSocket events (NodeEvent::Context) = node-to-frontend updates
///
/// Takes already-parsed events to avoid redundant deserialization
fn emit_state_mutation_event_parsed(
    node_client: &NodeClient,
    context_id: &ContextId,
    root_hash: Hash,
    events_payload: Vec<ExecutionEvent>,
) -> Result<()> {
    let state_mutation = ContextEvent {
        context_id: *context_id,
        payload: ContextEventPayload::StateMutation(StateMutationPayload::with_root_and_events(
            root_hash,
            events_payload,
        )),
    };

    if let Err(e) = node_client.send_event(NodeEvent::Context(state_mutation)) {
        warn!(
            %context_id,
            error = %e,
            "Failed to emit state mutation event to WebSocket clients"
        );
    }

    Ok(())
}

/// Requests missing parent deltas from a peer
///
/// Opens a stream to the peer and requests each missing delta sequentially.
/// Adds successfully retrieved deltas back to the DAG for processing.
async fn request_missing_deltas(
    network_client: calimero_network_primitives::client::NetworkClient,
    sync_timeout: std::time::Duration,
    context_id: ContextId,
    missing_ids: Vec<[u8; 32]>,
    source: PeerId,
    our_identity: PublicKey,
    delta_store: DeltaStore,
) -> Result<()> {
    use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

    // Open stream to peer
    let mut stream = network_client.open_stream(source).await?;

    // Request each missing delta
    for missing_id in missing_ids {
        info!(
            %context_id,
            delta_id = ?missing_id,
            "Requesting missing parent delta from peer"
        );

        // Send request
        let request_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::DeltaRequest {
                context_id,
                delta_id: missing_id,
            },
            next_nonce: {
                use rand::Rng;
                rand::thread_rng().gen()
            },
        };

        crate::sync::stream::send(&mut stream, &request_msg, None).await?;

        // Wait for response
        let timeout_budget = sync_timeout / 3;
        match crate::sync::stream::recv(&mut stream, None, timeout_budget).await? {
            Some(StreamMessage::Message {
                payload: MessagePayload::DeltaResponse { delta },
                ..
            }) => {
                // Deserialize storage delta
                let storage_delta: calimero_storage::delta::CausalDelta =
                    borsh::from_slice(&delta)?;

                info!(
                    %context_id,
                    delta_id = ?missing_id,
                    action_count = storage_delta.actions.len(),
                    "Received missing parent delta, adding to DAG"
                );

                // Convert to DAG delta
                let dag_delta = calimero_dag::CausalDelta {
                    id: storage_delta.id,
                    parents: storage_delta.parents,
                    payload: storage_delta.actions,
                    hlc: storage_delta.hlc,
                    expected_root_hash: storage_delta.expected_root_hash,
                };

                if let Err(e) = delta_store.add_delta(dag_delta).await {
                    warn!(?e, %context_id, delta_id = ?missing_id, "Failed to add requested delta");
                }
            }
            Some(StreamMessage::Message {
                payload: MessagePayload::DeltaNotFound,
                ..
            }) => {
                warn!(%context_id, delta_id = ?missing_id, "Peer doesn't have requested delta");
            }
            other => {
                warn!(%context_id, delta_id = ?missing_id, ?other, "Unexpected response to delta request");
            }
        }
    }

    Ok(())
}
