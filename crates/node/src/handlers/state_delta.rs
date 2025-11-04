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
use calimero_storage::action::Action;
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
    let mut author_identity = node_clients
        .context
        .get_identity(&context_id, &author_id)?
        .ok_or_eyre("author identity not found")?;

    // If we have the identity but missing sender_key, request it from any peer
    // All context members have all sender_keys (needed for gossip decryption)
    if author_identity.sender_key.is_none() {
        info!(
            %context_id,
            %author_id,
            "Missing sender_key for author - requesting from any available peer"
        );

        match request_sender_key_from_peers(
            &network_client,
            &node_clients.context,
            &context_id,
            &author_id,
            sync_timeout,
        )
        .await
        {
            Ok(sender_key) => {
                info!(
                    %context_id,
                    %author_id,
                    "Successfully fetched sender_key from peer"
                );
                author_identity.sender_key = Some(sender_key);
            }
            Err(e) => {
                warn!(
                    %context_id,
                    %author_id,
                    ?e,
                    "Failed to fetch sender_key from peers - delta will retry when rebroadcast"
                );
                bail!("author sender_key not available - request failed, will retry");
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

    let (delta_store_ref, is_new_store) = {
        let mut is_new = false;
        let delta_store = node_state
            .delta_stores
            .entry(context_id)
            .or_insert_with(|| {
                is_new = true;
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
        (delta_store_ref, is_new)
    };

    // Load persisted deltas on first access to restore DAG topology
    if is_new_store {
        if let Err(e) = delta_store_ref.load_persisted_deltas().await {
            warn!(
                ?e,
                %context_id,
                "Failed to load persisted deltas, starting with empty DAG"
            );
        }

        // After loading, check for missing parents and handle any cascaded events
        let missing_result = delta_store_ref.get_missing_parents().await;
        if !missing_result.missing_ids.is_empty() {
            warn!(
                %context_id,
                missing_count = missing_result.missing_ids.len(),
                "Missing parents after loading persisted deltas - will request from network"
            );

            // Note: We don't request here synchronously because we don't have the source peer
            // These will be requested when the next delta arrives or via periodic sync
        }

        // Execute handlers for any cascaded deltas from initial load
        if !missing_result.cascaded_events.is_empty() {
            info!(
                %context_id,
                cascaded_count = missing_result.cascaded_events.len(),
                "Executing event handlers for deltas cascaded during initial load"
            );

            for (cascaded_id, events_data) in missing_result.cascaded_events {
                match serde_json::from_slice::<Vec<ExecutionEvent>>(&events_data) {
                    Ok(cascaded_payload) => {
                        execute_event_handlers_parsed(
                            &node_clients.context,
                            &context_id,
                            &our_identity,
                            &cascaded_payload,
                        )
                        .await?;
                    }
                    Err(e) => {
                        warn!(%context_id, delta_id = ?cascaded_id, error = %e, "Failed to deserialize cascaded events from initial load");
                    }
                }
            }
        }
    }

    // Add delta with event data (for cascade handler execution)
    let add_result = delta_store_ref
        .add_delta_with_events(delta, events.clone())
        .await?;
    let mut applied = add_result.applied;

    // Track if we executed handlers for the current delta during cascade
    let mut handlers_already_executed = false;

    if !applied {
        // Delta is pending - check for missing parents
        let missing_result = delta_store_ref.get_missing_parents().await;

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
                            &node_clients.context,
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
                context_is_uninitialized = is_uninitialized,
                has_events = events.is_some(),
                "Delta pending due to missing parents - requesting them from peer"
            );

            // Request missing deltas (blocking this handler until complete)
            if let Err(e) = request_missing_deltas(
                network_client,
                sync_timeout,
                context_id,
                missing_result.missing_ids,
                source,
                our_identity,
                delta_store_ref.clone(),
            )
            .await
            {
                warn!(?e, %context_id, ?source, "Failed to request missing deltas");
            }
        } else {
            // No missing parents - the parent deltas exist but may not be applied yet
            // This can happen when deltas arrive out of order via gossipsub
            // The delta will cascade and apply when its parents finish applying
            warn!(
                %context_id,
                delta_id = ?delta_id,
                has_events = events.is_some(),
                "Delta pending - parents exist but not yet applied (will cascade when ready)"
            );
        }

        // Always re-check if delta was applied via cascade (can happen during request_missing_deltas OR gossipsub)
        let was_cascaded = delta_store_ref.dag_has_delta_applied(&delta_id).await;
        if was_cascaded {
            info!(
                %context_id,
                delta_id = ?delta_id,
                "Delta was applied via cascade - will execute handlers"
            );
            applied = true;

            // Important: If delta cascaded but we haven't executed handlers yet,
            // and we have events, we need to execute them now.
            // This can happen if the cascade occurred via another concurrent handler.
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
    // Note: Handlers are only executed on receiving nodes, not on the author node,
    // to avoid infinite loops and ensure proper distributed execution.
    if applied && !handlers_already_executed {
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
                        &node_clients.context,
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

    // Fetch all missing ancestors, then add them in topological order (oldest first)
    let mut to_fetch = missing_ids;
    let mut fetched_deltas: Vec<(calimero_dag::CausalDelta<Vec<Action>>, [u8; 32])> = Vec::new();
    let mut fetch_count = 0;

    // Phase 1: Fetch ALL missing deltas recursively
    // No artificial limit - DAG is acyclic so this will naturally terminate at genesis
    while !to_fetch.is_empty() {
        let current_batch = to_fetch.clone();
        to_fetch.clear();

        for missing_id in current_batch {
            fetch_count += 1;

            info!(
                %context_id,
                delta_id = ?missing_id,
                total_fetched = fetch_count,
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
                        "Received missing parent delta"
                    );

                    // Convert to DAG delta
                    let dag_delta = calimero_dag::CausalDelta {
                        id: storage_delta.id,
                        parents: storage_delta.parents.clone(),
                        payload: storage_delta.actions,
                        hlc: storage_delta.hlc,
                        expected_root_hash: storage_delta.expected_root_hash,
                    };

                    // Store for later (don't add to DAG yet!)
                    fetched_deltas.push((dag_delta, missing_id));

                    // Check what parents THIS delta needs
                    for parent_id in &storage_delta.parents {
                        // Skip genesis
                        if *parent_id == [0; 32] {
                            continue;
                        }
                        // Skip if we already have it or are about to fetch it
                        if !delta_store.has_delta(parent_id).await
                            && !to_fetch.contains(parent_id)
                            && !fetched_deltas.iter().any(|(d, _)| d.id == *parent_id)
                        {
                            to_fetch.push(*parent_id);
                        }
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
    }

    // Phase 2: Add all fetched deltas to DAG in topological order (oldest first)
    // We fetched breadth-first, so reversing gives us depth-first (ancestors before descendants)
    if !fetched_deltas.is_empty() {
        info!(
            %context_id,
            total_fetched = fetched_deltas.len(),
            "Adding fetched deltas to DAG in topological order"
        );

        // Reverse so oldest ancestors are added first
        fetched_deltas.reverse();

        for (dag_delta, delta_id) in fetched_deltas {
            if let Err(e) = delta_store.add_delta(dag_delta).await {
                warn!(?e, %context_id, delta_id = ?delta_id, "Failed to add fetched delta to DAG");
            }
        }

        // Log warning for very large syncs (informational, not a hard limit)
        if fetch_count > 1000 {
            warn!(
                %context_id,
                total_fetched = fetch_count,
                "Large sync detected - fetched many deltas from peer (context has deep history)"
            );
        }
    }

    Ok(())
}

/// Request sender_key for a member from any available peer
/// This is a simple request/response (not bidirectional), avoiding deadlocks
async fn request_sender_key_from_peers(
    network_client: &calimero_network_primitives::client::NetworkClient,
    context_client: &ContextClient,
    context_id: &ContextId,
    member_id: &PublicKey,
    timeout: std::time::Duration,
) -> Result<calimero_primitives::identity::PrivateKey> {
    use calimero_network_primitives::messages::TopicHash;
    use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
    use rand::Rng;

    // Get all peers from the context mesh
    let peers = network_client
        .mesh_peers(TopicHash::from_raw(*context_id))
        .await;

    if peers.is_empty() {
        bail!("No peers available to request sender_key from");
    }

    // Try each peer until one responds successfully
    for peer in peers {
        debug!(
            %context_id,
            %member_id,
            %peer,
            "Requesting sender_key from peer"
        );

        // Wrap entire attempt in single timeout to avoid double-counting
        let result = tokio::time::timeout(timeout, async {
            // Open stream
            let mut stream = network_client.open_stream(peer).await?;

            // Get our identity
            let identities = context_client.get_context_members(context_id, Some(true));
            let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
                .await
                .transpose()?
            else {
                bail!("no owned identities found for context");
            };

            // Send request
            crate::sync::stream::send(
                &mut stream,
                &StreamMessage::Init {
                    context_id: *context_id,
                    party_id: our_identity,
                    payload: InitPayload::IdentityRequest {
                        context_id: *context_id,
                        identity: *member_id,
                    },
                    next_nonce: rand::thread_rng().gen(),
                },
                None,
            )
            .await?;

            // Receive response
            let response = crate::sync::stream::recv(&mut stream, None, timeout)
                .await?
                .ok_or_eyre("peer closed stream without responding")?;

            Ok::<_, eyre::Report>(response)
        })
        .await;

        let response = match result {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => {
                warn!(%peer, ?e, "Failed to request sender_key from peer");
                continue;
            }
            Err(_) => {
                warn!(%peer, "Timeout requesting sender_key from peer");
                continue;
            }
        };

        // Parse response
        match response {
            StreamMessage::Message {
                payload:
                    MessagePayload::IdentityResponse {
                        identity: resp_identity,
                        sender_key: Some(sender_key),
                    },
                ..
            } if resp_identity == *member_id => {
                info!(
                    %context_id,
                    %member_id,
                    %peer,
                    "Received sender_key from peer - updating local identity store"
                );

                // Update local identity store
                let mut local_identity = context_client
                    .get_identity(context_id, member_id)?
                    .ok_or_eyre("identity disappeared during request")?;

                local_identity.sender_key = Some(sender_key);
                context_client.update_identity(context_id, &local_identity)?;

                return Ok(local_identity
                    .sender_key
                    .ok_or_eyre("sender_key disappeared after update")?);
            }
            StreamMessage::Message {
                payload:
                    MessagePayload::IdentityResponse {
                        sender_key: None, ..
                    },
                ..
            } => {
                warn!(%peer, %member_id, "Peer doesn't have sender_key for this member");
                continue;
            }
            other => {
                warn!(%peer, ?other, "Unexpected response to sender_key request");
                continue;
            }
        }
    }

    bail!("No peers could provide sender_key for member {}", member_id);
}
