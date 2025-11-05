//! State Delta Handler - Stateless Gossipsub broadcast handler
//!
//! **Purpose**: Process state delta broadcasts from gossipsub network.
//!
//! **Protocol**:
//! 1. Receive delta broadcast (encrypted artifact)
//! 2. Request sender_key if missing (via key_exchange protocol)
//! 3. Decrypt and validate delta
//! 4. Add to DAG (cascades if parents ready, pends otherwise)
//! 5. Request missing parents if needed
//! 6. Execute event handlers for applied deltas
//! 7. Emit to WebSocket clients (frontends)
//!
//! **Stateless Design**: All dependencies injected as parameters!
//!
//! **Note**: This uses DeltaStore for DAG operations - DeltaStore is NOT stateless
//! (it's a stateful in-memory DAG), but this HANDLER is stateless (pure function).

use calimero_context_primitives::client::ContextClient;
use calimero_crypto::Nonce;
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, NodeEvent, StateMutationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt, Result};
use libp2p::PeerId;
use tokio::time::Duration;
use tracing::{debug, info, warn};

// Re-export types that need to be provided by calimero-node
pub use crate::p2p::delta_request::{AddDeltaResult, DeltaStore, MissingParentsResult};
pub use crate::p2p::key_exchange::{handle_key_exchange, request_key_exchange};

// ═══════════════════════════════════════════════════════════════════════════
// Main Handler: Process State Delta Broadcast
// ═══════════════════════════════════════════════════════════════════════════

/// Handle state delta received from a peer via gossipsub (stateless).
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
///
/// # Arguments
/// * `node_client` - Client for node operations (events, blobs)
/// * `context_client` - Client for context operations (identities, contexts)
/// * `network_client` - Client for network operations (streams)
/// * `delta_store` - DeltaStore for this context (injected!)
/// * `our_identity` - Our identity in this context
/// * `sync_timeout` - Timeout for sync operations
/// * `source` - Peer ID of the broadcaster
/// * `context_id` - Context ID for the delta
/// * `author_id` - Author of the delta
/// * `delta_id` - ID of the delta
/// * `parent_ids` - Parent delta IDs
/// * `hlc` - Hybrid logical clock timestamp
/// * `root_hash` - Expected root hash after applying
/// * `artifact` - Encrypted delta payload
/// * `nonce` - Nonce for decryption
/// * `events` - Optional serialized events
#[allow(clippy::too_many_arguments)]
pub async fn handle_state_delta(
    node_client: &NodeClient,
    context_client: &ContextClient,
    network_client: &NetworkClient,
    delta_store: &dyn DeltaStore,
    our_identity: PublicKey,
    sync_timeout: Duration,
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
    let Some(context) = context_client.get_context(&context_id)? else {
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
    let mut author_identity = context_client
        .get_identity(&context_id, &author_id)?
        .ok_or_eyre("author identity not found")?;

    // If we have the identity but missing sender_key, do direct key share with source peer
    // Note: We check again here to avoid concurrent key exchanges for the same author
    if author_identity.sender_key.is_none() {
        // Re-check identity in case another concurrent delta handler just completed key exchange
        author_identity = context_client
            .get_identity(&context_id, &author_id)?
            .ok_or_eyre("author identity not found")?;
            
        // Skip if key was just added by concurrent handler
        if author_identity.sender_key.is_some() {
            debug!(
                %context_id,
                %author_id,
                "sender_key now available (concurrent handler completed key exchange)"
            );
        } else {
            info!(
                %context_id,
                %author_id,
                source_peer=%source,
                "Missing sender_key for author - initiating key share with source peer"
            );

            match request_key_share_with_peer(
            network_client,
            context_client,
            &context_id,
            &author_id,
            source,
            sync_timeout,
        )
        .await
        {
            Ok(()) => {
                info!(
                    %context_id,
                    %author_id,
                    source_peer=%source,
                    "Successfully completed key share with source peer"
                );
                // Reload identity to get the updated sender_key
                author_identity = context_client
                    .get_identity(&context_id, &author_id)?
                    .ok_or_eyre("author identity disappeared")?;
            }
            Err(e) => {
                warn!(
                    %context_id,
                    %author_id,
                    source_peer=%source,
                    ?e,
                    "Failed to complete key share with source peer - will retry when delta is rebroadcast"
                );
                bail!("author sender_key not available (key share requested, will retry)");
            }
            }
        }
    }

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
        payload: actions,
        hlc,
        expected_root_hash: *root_hash,
    };

    // Add delta with event data (for cascade handler execution)
    let add_result = delta_store
        .add_delta_with_events(delta, events.clone())
        .await?;
    let mut applied = add_result.applied;

    // Track if we executed handlers for the current delta during cascade
    let mut handlers_already_executed = false;

    if !applied {
        // Delta is pending - check for missing parents
        let missing_result = delta_store.get_missing_parents().await;

        // Execute handlers for cascaded deltas from DB load (including this delta if it cascaded)
        if !missing_result.cascaded_events.is_empty() {
            info!(
                %context_id,
                cascaded_count = missing_result.cascaded_events.len(),
                "Executing event handlers for deltas cascaded during missing parent check"
            );

            for (cascaded_id, events_data) in &missing_result.cascaded_events {
                // Check if this is the current delta that cascaded
                let is_current_delta = *cascaded_id == delta_id;
                if is_current_delta {
                    info!(
                        %context_id,
                        delta_id = ?delta_id,
                        "Current delta cascaded during missing parent check - marking as applied"
                    );
                    applied = true;
                }

                match serde_json::from_slice::<Vec<ExecutionEvent>>(events_data) {
                    Ok(cascaded_payload) => {
                        info!(
                            %context_id,
                            delta_id = ?cascaded_id,
                            events_count = cascaded_payload.len(),
                            "Executing handlers for cascaded delta"
                        );
                        execute_event_handlers_parsed(
                            context_client,
                            &context_id,
                            &our_identity,
                            &cascaded_payload,
                        )
                        .await?;

                        // Mark that we executed handlers for the current delta
                        if is_current_delta {
                            handlers_already_executed = true;
                        }
                    }
                    Err(e) => {
                        warn!(%context_id, delta_id = ?cascaded_id, error = %e, "Failed to deserialize cascaded events");
                    }
                }
            }
        }

        if !missing_result.missing_ids.is_empty() {
            warn!(
                %context_id,
                missing_count = missing_result.missing_ids.len(),
                has_events = events.is_some(),
                "Delta pending due to missing parents - requesting them from peer"
            );

            // Request missing deltas (blocking this handler until complete)
            if let Err(e) = crate::p2p::delta_request::request_missing_deltas(
                network_client,
                context_id,
                missing_result.missing_ids,
                source,
                delta_store,
                our_identity,
                context_client,
                sync_timeout,
            )
            .await
            {
                warn!(?e, %context_id, ?source, "Failed to request missing deltas");
            }
        } else {
            // No missing parents - the parent deltas exist but may not be applied yet
            warn!(
                %context_id,
                delta_id = ?delta_id,
                has_events = events.is_some(),
                "Delta pending - parents exist but not yet applied (will cascade when ready)"
            );
        }

        // Always re-check if delta was applied via cascade
        let was_cascaded = delta_store.dag_has_delta_applied(&delta_id).await;
        if was_cascaded {
            info!(
                %context_id,
                delta_id = ?delta_id,
                "Delta was applied via cascade - will execute handlers"
            );
            applied = true;

            if !handlers_already_executed && events.is_some() {
                info!(
                    %context_id,
                    delta_id = ?delta_id,
                    "Delta cascaded via alternate path - handlers will be executed in main flow"
                );
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

    // Execute event handlers only if the delta was applied AND we haven't already executed them
    // Note: Handlers are only executed on receiving nodes, not on the author node
    if applied && !handlers_already_executed {
        if let Some(ref payload) = events_payload {
            if author_id != our_identity {
                info!(
                    %context_id,
                    %author_id,
                    %our_identity,
                    "Executing event handlers (delta applied, we are a receiving node)"
                );
                execute_event_handlers_parsed(context_client, &context_id, &our_identity, payload)
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
    if let Some(payload) = events_payload {
        emit_state_mutation_event_parsed(node_client, &context_id, root_hash, payload)?;
    }

    // Execute handlers for any cascaded deltas that had stored events
    if !add_result.cascaded_events.is_empty() {
        info!(
            %context_id,
            cascaded_count = add_result.cascaded_events.len(),
            "Executing event handlers for cascaded deltas"
        );

        for (cascaded_id, events_data) in add_result.cascaded_events {
            match serde_json::from_slice::<Vec<ExecutionEvent>>(&events_data) {
                Ok(cascaded_payload) => {
                    info!(
                        %context_id,
                        delta_id = ?cascaded_id,
                        events_count = cascaded_payload.len(),
                        "Executing handlers for cascaded delta"
                    );
                    execute_event_handlers_parsed(
                        context_client,
                        &context_id,
                        &our_identity,
                        &cascaded_payload,
                    )
                    .await?;
                }
                Err(e) => {
                    warn!(
                        %context_id,
                        delta_id = ?cascaded_id,
                        error = %e,
                        "Failed to deserialize cascaded events, skipping handler execution"
                    );
                }
            }
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper: Execute Event Handlers
// ═══════════════════════════════════════════════════════════════════════════

/// Execute event handlers for received events (from already-parsed payload).
///
/// # Handler Execution Model
///
/// Handlers currently execute **sequentially** in the order they appear
/// in the events array. Future optimization may execute handlers in **parallel**.
///
/// ## Requirements for Application Handlers
///
/// Event handlers **MUST** satisfy these properties to be correct:
///
/// 1. **Commutative**: Handler order must not affect final state
/// 2. **Independent**: Handlers must not share mutable state
/// 3. **Idempotent**: Re-execution must be safe
/// 4. **No side effects**: Handlers should only modify CRDT state
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

// ═══════════════════════════════════════════════════════════════════════════
// Helper: Emit WebSocket Events
// ═══════════════════════════════════════════════════════════════════════════

/// Emit state mutation event to WebSocket clients (frontends).
///
/// Note: This is separate from node-to-node DAG synchronization.
/// - DAG broadcast (BroadcastMessage::StateDelta) = node-to-node sync
/// - WebSocket events (NodeEvent::Context) = node-to-frontend updates
///
/// Takes already-parsed events to avoid redundant deserialization.
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

// ═══════════════════════════════════════════════════════════════════════════
// Helper: Request Key Share
// ═══════════════════════════════════════════════════════════════════════════

/// Request key share with a peer (upgraded to use SecureStream with full challenge-response auth).
///
/// **Security Upgrade**: Previous implementation just exchanged sender_keys without authentication.
/// Now uses `authenticate_p2p()` which includes:
/// - Bidirectional challenge-response authentication (prevents impersonation)
/// - Signature verification
/// - Deadlock prevention
///
/// Request key share from a peer when we're missing the sender_key for an author.
///
/// This uses the P2P key exchange protocol to establish shared encryption keys
/// with the peer, allowing us to decrypt future deltas from that author.
///
/// The function:
/// 1. Gets our owned identity from the context
/// 2. Opens a P2P stream to the peer
/// 3. Performs mutual key exchange using the secure key exchange protocol
async fn request_key_share_with_peer(
    network_client: &NetworkClient,
    context_client: &ContextClient,
    context_id: &ContextId,
    _author_identity: &PublicKey,
    peer: PeerId,
    timeout: Duration,
) -> Result<()> {
    use futures_util::stream::StreamExt;
    use std::pin::pin;

    // Get our owned identity for this context
    let mut members_stream = pin!(context_client.get_context_members(context_id, Some(true))); // owned=true
    let our_identity = members_stream
        .next()
        .await
        .ok_or_eyre("No owned identity found in context")?? // outer ? for stream error, inner ? for Result
        .0; // Extract PublicKey from (PublicKey, bool) tuple

    // Get the full context object (needed for key exchange)
    let context = context_client
        .get_context(context_id)?
        .ok_or_eyre("Context not found")?;

    // Use the P2P key exchange protocol
    crate::p2p::key_exchange::request_key_exchange(
        network_client,
        &context,
        our_identity,
        peer,
        context_client,
        timeout,
    )
    .await
}
