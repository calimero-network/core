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
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::action::Action;
use eyre::{bail, OptionExt, Result};
use libp2p::PeerId;
use tracing::{debug, info, warn};

use crate::delta_store::DeltaStore;
use crate::sync::CHALLENGE_DOMAIN;
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

    // Check if we should buffer this delta (during snapshot sync)
    if node_state.should_buffer_delta(&context_id) {
        info!(
            %context_id,
            delta_id = ?delta_id,
            "Buffering delta during snapshot sync"
        );
        let buffered = calimero_node_primitives::sync_protocol::BufferedDelta {
            id: delta_id,
            parents: parent_ids.clone(),
            hlc: hlc.get_time().as_u64(),
            payload: artifact.clone(), // Store encrypted payload for replay
        };
        if node_state.buffer_delta(&context_id, buffered) {
            return Ok(()); // Successfully buffered, will be replayed after snapshot
        } else {
            warn!(
                %context_id,
                delta_id = ?delta_id,
                "Delta buffer full, proceeding with normal processing"
            );
            // Fall through to normal processing
        }
    }

    let sender_key = ensure_author_sender_key(
        &node_clients.context,
        &network_client,
        &context_id,
        &author_id,
        source,
        sync_timeout,
        context.root_hash,
    )
    .await?;

    let actions = decrypt_delta_actions(artifact, nonce, sender_key)?;

    let delta = calimero_dag::CausalDelta {
        id: delta_id,
        parents: parent_ids,
        payload: actions,
        hlc,
        expected_root_hash: *root_hash,
    };

    let our_identity = choose_owned_identity(&node_clients.context, &context_id).await?;

    // Check if application is available BEFORE applying the delta.
    // If not available, bail early so the delta can be retried later when rebroadcast.
    // This prevents the scenario where we apply the delta but skip handlers because
    // the application blob hasn't finished downloading yet.
    if let Err(e) = ensure_application_available(
        &node_clients.node,
        &node_clients.context,
        &context_id,
        sync_timeout,
    )
    .await
    {
        bail!(
            "Application not available for context {} - delta will be retried on rebroadcast: {}",
            context_id,
            e
        );
    }

    let DeltaStoreSetup {
        store: delta_store_ref,
        is_uninitialized,
    } = init_delta_store(
        &node_state,
        &node_clients,
        context_id,
        our_identity,
        context.root_hash,
        sync_timeout,
    )
    .await?;

    let add_result = delta_store_ref
        .add_delta_with_events(delta, events.clone())
        .await?;
    let mut applied = add_result.applied;
    let mut handlers_already_executed = false;

    if !applied {
        let missing_result = delta_store_ref.get_missing_parents().await;

        let cascade_outcome = execute_cascaded_events(
            &missing_result.cascaded_events,
            &node_clients,
            &context_id,
            &our_identity,
            sync_timeout,
            "missing parent check",
            Some(&delta_id),
        )
        .await?;
        applied |= cascade_outcome.applied_current;
        handlers_already_executed |= cascade_outcome.handlers_executed_for_current;

        if !missing_result.missing_ids.is_empty() {
            warn!(
                %context_id,
                missing_count = missing_result.missing_ids.len(),
                context_is_uninitialized = is_uninitialized,
                has_events = events.is_some(),
                "Delta pending due to missing parents - requesting them from peer"
            );

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
            warn!(
                %context_id,
                delta_id = ?delta_id,
                has_events = events.is_some(),
                "Delta pending - parents exist but not yet applied (will cascade when ready)"
            );
        }

        let was_cascaded = delta_store_ref.dag_has_delta_applied(&delta_id).await;
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

    let events_payload = parse_events_payload(&events, &context_id);

    if applied && !handlers_already_executed {
        if let Some(ref payload) = events_payload {
            let is_author = author_id == our_identity;
            info!(
                %context_id,
                %author_id,
                %our_identity,
                is_author,
                "Evaluating event handler execution for applied delta"
            );
            if !is_author {
                // Application availability was already verified at the start of this function,
                // so we can safely execute handlers without re-checking.
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
    } else if !applied && events_payload.is_some() {
        warn!(
            %context_id,
            delta_id = ?delta_id,
            "Delta with events buffered as pending - handlers will NOT execute when delta is applied later!"
        );
    }

    if let Some(payload) = events_payload {
        emit_state_mutation_event_parsed(&node_clients.node, &context_id, root_hash, payload)?;
    }

    execute_cascaded_events(
        &add_result.cascaded_events,
        &node_clients,
        &context_id,
        &our_identity,
        sync_timeout,
        "dag cascade",
        None,
    )
    .await?;

    Ok(())
}

#[derive(Default)]
struct CascadeOutcome {
    applied_current: bool,
    handlers_executed_for_current: bool,
}

struct DeltaStoreSetup {
    store: DeltaStore,
    is_uninitialized: bool,
}

fn decrypt_delta_actions(
    artifact: Vec<u8>,
    nonce: Nonce,
    sender_key: PrivateKey,
) -> Result<Vec<Action>> {
    let shared_key = calimero_crypto::SharedKey::from_sk(&sender_key);
    let decrypted_artifact = shared_key
        .decrypt(artifact, nonce)
        .ok_or_eyre("failed to decrypt artifact")?;

    let storage_delta: calimero_storage::delta::StorageDelta =
        borsh::from_slice(&decrypted_artifact)?;

    match storage_delta {
        calimero_storage::delta::StorageDelta::Actions(actions) => Ok(actions),
        _ => bail!("Expected Actions variant in state delta"),
    }
}

async fn ensure_author_sender_key(
    context_client: &ContextClient,
    network_client: &calimero_network_primitives::client::NetworkClient,
    context_id: &ContextId,
    author_id: &PublicKey,
    source: PeerId,
    sync_timeout: std::time::Duration,
    context_root_hash: Hash,
) -> Result<PrivateKey> {
    let mut author_identity = context_client
        .get_identity(context_id, author_id)?
        .ok_or_eyre("author identity not found")?;

    if author_identity.sender_key.is_none() {
        // Check if context is uninitialized (bootstrapping)
        // During bootstrap, skip key share to avoid blocking state sync.
        // The key share would block for ~350ms and interfere with the parallel
        // state sync stream from the same peer. State sync will deliver all
        // deltas needed to initialize the context. Once initialized, subsequent
        // deltas can trigger key shares without blocking critical bootstrap.
        if context_root_hash == Hash::default() {
            debug!(
                %context_id,
                %author_id,
                source_peer=%source,
                "Context uninitialized - deferring key share until after state sync completes"
            );
            bail!("author sender_key not available (context uninitialized, deferring key share)");
        }

        info!(
            %context_id,
            %author_id,
            source_peer=%source,
            "Missing sender_key for author - initiating key share with source peer"
        );

        match request_key_share_with_peer(
            network_client,
            context_client,
            context_id,
            author_id,
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
                author_identity = context_client
                    .get_identity(context_id, author_id)?
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

    author_identity
        .sender_key
        .ok_or_eyre("author has no sender key")
}

async fn choose_owned_identity(
    context_client: &ContextClient,
    context_id: &ContextId,
) -> Result<PublicKey> {
    let identities = context_client.get_context_members(context_id, Some(true));
    let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
        .await
        .transpose()?
    else {
        bail!("no owned identities found for context: {}", context_id);
    };

    Ok(our_identity)
}

async fn init_delta_store(
    node_state: &crate::NodeState,
    node_clients: &crate::NodeClients,
    context_id: ContextId,
    our_identity: PublicKey,
    root_hash: Hash,
    sync_timeout: std::time::Duration,
) -> Result<DeltaStoreSetup> {
    let is_uninitialized = root_hash == Hash::default();

    let (delta_store_ref, is_new_store) = {
        let mut is_new = false;
        let delta_store = node_state
            .delta_stores
            .entry(context_id)
            .or_insert_with(|| {
                is_new = true;
                DeltaStore::new(
                    [0u8; 32],
                    node_clients.context.clone(),
                    context_id,
                    our_identity,
                )
            });

        (delta_store.clone(), is_new)
    };

    if is_new_store {
        let init_result = async {
            if let Err(e) = delta_store_ref.load_persisted_deltas().await {
                warn!(
                    ?e,
                    %context_id,
                    "Failed to load persisted deltas, starting with empty DAG"
                );
            }

            let missing_result = delta_store_ref.get_missing_parents().await;
            if !missing_result.missing_ids.is_empty() {
                warn!(
                    %context_id,
                    missing_count = missing_result.missing_ids.len(),
                    "Missing parents after loading persisted deltas - will request from network"
                );
            }

            execute_cascaded_events(
                &missing_result.cascaded_events,
                node_clients,
                &context_id,
                &our_identity,
                sync_timeout,
                "initial load",
                None,
            )
            .await
        }
        .await;

        if let Err(err) = init_result {
            warn!(
                %context_id,
                ?err,
                "Initial delta store setup failed - removing store to retry on next delta"
            );
            // Remove the store so the next delta triggers a fresh init with retry
            node_state.delta_stores.remove(&context_id);
            return Err(err);
        }
    }

    Ok(DeltaStoreSetup {
        store: delta_store_ref,
        is_uninitialized,
    })
}

async fn execute_cascaded_events(
    cascaded_events: &[([u8; 32], Vec<u8>)],
    node_clients: &crate::NodeClients,
    context_id: &ContextId,
    our_identity: &PublicKey,
    sync_timeout: std::time::Duration,
    phase: &str,
    current_delta: Option<&[u8; 32]>,
) -> Result<CascadeOutcome> {
    if cascaded_events.is_empty() {
        return Ok(CascadeOutcome::default());
    }

    info!(
        %context_id,
        cascaded_count = cascaded_events.len(),
        phase = phase,
        "Executing event handlers for cascaded deltas"
    );

    let mut outcome = CascadeOutcome::default();

    // Check if current delta is in cascaded list (orthogonal to handler execution)
    if let Some(current) = current_delta {
        if cascaded_events.iter().any(|(id, _)| *id == *current) {
            info!(
                %context_id,
                delta_id = ?current,
                phase = phase,
                "Current delta cascaded - marking as applied"
            );
            outcome.applied_current = true;
        }
    }

    let app_available = ensure_application_available(
        &node_clients.node,
        &node_clients.context,
        context_id,
        sync_timeout,
    )
    .await
    .is_ok();

    if !app_available {
        warn!(
            %context_id,
            cascaded_count = cascaded_events.len(),
            phase = phase,
            "Application not available - skipping cascaded handler execution"
        );
        return Ok(outcome);
    }

    for (cascaded_id, events_data) in cascaded_events {
        match serde_json::from_slice::<Vec<ExecutionEvent>>(events_data) {
            Ok(cascaded_payload) => {
                info!(
                    %context_id,
                    delta_id = ?cascaded_id,
                    events_count = cascaded_payload.len(),
                    phase = phase,
                    "Executing handlers for cascaded delta"
                );
                execute_event_handlers_parsed(
                    &node_clients.context,
                    context_id,
                    our_identity,
                    &cascaded_payload,
                )
                .await?;

                if current_delta == Some(cascaded_id) {
                    outcome.handlers_executed_for_current = true;
                }
            }
            Err(e) => {
                warn!(
                    %context_id,
                    delta_id = ?cascaded_id,
                    error = %e,
                    phase = phase,
                    "Failed to deserialize cascaded events"
                );
            }
        }
    }

    Ok(outcome)
}

fn parse_events_payload(
    events: &Option<Vec<u8>>,
    context_id: &ContextId,
) -> Option<Vec<ExecutionEvent>> {
    let Some(events_data) = events else {
        return None;
    };

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_crypto::{SharedKey, NONCE_LEN};
    use calimero_storage::delta::StorageDelta;
    use rand::thread_rng;

    #[test]
    fn parse_events_payload_success() {
        let events = vec![ExecutionEvent {
            kind: "test".to_string(),
            data: vec![1, 2, 3],
            handler: Some("handler_fn".to_string()),
        }];
        let serialized = serde_json::to_vec(&events).expect("serialization should succeed");

        // Should deserialize valid event JSON
        let parsed = parse_events_payload(&Some(serialized), &ContextId::zero())
            .expect("events should parse");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].kind, "test");
        assert_eq!(parsed[0].handler.as_deref(), Some("handler_fn"));
    }

    #[test]
    fn parse_events_payload_invalid() {
        // Invalid JSON should be rejected gracefully
        let parsed = parse_events_payload(&Some(b"not-json".to_vec()), &ContextId::zero());
        assert!(parsed.is_none());
    }

    #[test]
    fn decrypt_delta_actions_roundtrip() -> Result<()> {
        let mut rng = thread_rng();
        let sender_key = PrivateKey::random(&mut rng);
        let shared_key = SharedKey::from_sk(&sender_key);
        let nonce = [7u8; NONCE_LEN];

        let storage_delta = StorageDelta::Actions(Vec::new());
        let plaintext = borsh::to_vec(&storage_delta)?;
        let cipher = shared_key
            .encrypt(plaintext, nonce)
            .ok_or_eyre("encryption failed")?;

        // Encrypted storage delta should decrypt back to empty actions
        let decrypted = decrypt_delta_actions(cipher, nonce, sender_key)?;
        assert!(decrypted.is_empty());

        Ok(())
    }

    #[test]
    fn decrypt_delta_actions_rejects_bad_cipher() {
        let mut rng = thread_rng();
        let sender_key = PrivateKey::random(&mut rng);
        let nonce = [9u8; NONCE_LEN];

        // Garbage ciphertext should fail to decrypt/deserialize
        let result = decrypt_delta_actions(vec![1, 2, 3, 4], nonce, sender_key);
        assert!(result.is_err());
    }
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

/// Initiate bidirectional key share with a specific peer for a specific author identity
/// This performs the same cryptographic key exchange as initial sync, but on-demand.
///
/// **IMPORTANT**: This must follow the exact same protocol as `sync/key.rs::bidirectional_key_share`
/// to be compatible with the responder side (`handle_key_share_request`).
///
/// Protocol:
/// 1. Init (KeyShare) → Init (KeyShare) ack
/// 2. Challenge/Response authentication (deterministic initiator/responder roles)
/// 3. KeyShare message exchange (all messages unencrypted - transport layer handles encryption)
async fn request_key_share_with_peer(
    network_client: &calimero_network_primitives::client::NetworkClient,
    context_client: &ContextClient,
    context_id: &ContextId,
    author_identity: &PublicKey,
    peer: PeerId,
    timeout: std::time::Duration,
) -> Result<()> {
    use calimero_crypto::Nonce;
    use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
    use ed25519_dalek::Signature;
    use rand::Rng;

    debug!(
        %context_id,
        %author_identity,
        %peer,
        "Initiating bidirectional key share with peer"
    );

    // Wrap entire key share in single timeout
    tokio::time::timeout(timeout, async {
        // Open stream to source peer
        let mut stream = network_client.open_stream(peer).await?;

        // Get our identity for this context
        let identities = context_client.get_context_members(context_id, Some(true));
        let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context");
        };

        let our_nonce = rand::thread_rng().gen::<Nonce>();

        // Step 1: Initiate key share request
        crate::sync::stream::send(
            &mut stream,
            &StreamMessage::Init {
                context_id: *context_id,
                party_id: our_identity,
                payload: InitPayload::KeyShare,
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        // Step 2: Receive ack from peer (contains their identity)
        let Some(ack) = crate::sync::stream::recv(&mut stream, None, timeout).await? else {
            bail!("connection closed while awaiting key share handshake");
        };

        let their_identity = match ack {
            StreamMessage::Init {
                party_id,
                payload: InitPayload::KeyShare,
                ..
            } => party_id,
            unexpected => {
                bail!("unexpected message during key share: {:?}", unexpected)
            }
        };

        // Verify peer responded with the identity we need
        // If the peer has multiple identities, they might respond with a different one
        if their_identity != *author_identity {
            warn!(
                %context_id,
                %author_identity,
                %their_identity,
                "Peer responded with different identity than expected author - key share may not provide the needed sender_key"
            );
            bail!(
                "peer responded with unexpected identity (expected author {}, got {})",
                author_identity,
                their_identity
            );
        }

        // Step 3: Deterministic tie-breaker for initiator/responder role
        // Both peers must agree on roles to prevent deadlock
        let is_initiator = <PublicKey as AsRef<[u8; 32]>>::as_ref(&our_identity)
            > <PublicKey as AsRef<[u8; 32]>>::as_ref(&their_identity);

        debug!(
            %context_id,
            %our_identity,
            %their_identity,
            is_initiator,
            "Determined role via deterministic comparison"
        );

        // Get our private key and sender key
        let (our_private_key, sender_key) = context_client
            .get_identity(context_id, &our_identity)?
            .and_then(|i| Some((i.private_key?, i.sender_key?)))
            .ok_or_eyre("expected own identity to have private & sender keys")?;

        // Get their identity record (to update with sender_key later)
        let mut their_identity_record = context_client
            .get_identity(context_id, &their_identity)?
            .ok_or_eyre("expected peer identity to exist")?;

        let mut sequence_id_out: usize = 0;
        let mut sequence_id_in: usize = 0;

        // Step 4: Challenge/Response authentication
        // Protocol must match sync/key.rs exactly:
        // - Initiator: send challenge → recv response → recv challenge → send response
        // - Responder: recv challenge → send response → send challenge → recv response
        if is_initiator {
            // INITIATOR: Challenge them first
            let challenge: [u8; 32] = rand::thread_rng().gen();

            debug!(%context_id, %their_identity, "Sending authentication challenge (initiator)");

            crate::sync::stream::send(
                &mut stream,
                &StreamMessage::Message {
                    sequence_id: sequence_id_out,
                    payload: MessagePayload::Challenge { challenge },
                    next_nonce: our_nonce,
                },
                None,
            )
            .await?;
            sequence_id_out += 1;

            // Receive their signature
            let Some(msg) = crate::sync::stream::recv(&mut stream, None, timeout).await? else {
                bail!("connection closed while awaiting challenge response");
            };

            let their_signature_bytes = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::ChallengeResponse { signature },
                    ..
                } => {
                    if sequence_id != sequence_id_in {
                        bail!(
                            "unexpected sequence_id: expected {}, got {}",
                            sequence_id_in,
                            sequence_id
                        );
                    }
                    sequence_id_in += 1;
                    signature
                }
                unexpected => bail!("expected ChallengeResponse, got {:?}", unexpected),
            };

            let mut peer_payload = CHALLENGE_DOMAIN.to_vec();
            peer_payload.extend_from_slice(&challenge);

            // Verify their signature
            let their_signature = Signature::from_bytes(&their_signature_bytes);
            their_identity
                .verify(&peer_payload, &their_signature)
                .map_err(|e| eyre::eyre!("Peer failed to prove identity ownership: {}", e))?;

            debug!(%context_id, %their_identity, "Peer authenticated successfully");

            // Now receive their challenge for us
            let Some(msg) = crate::sync::stream::recv(&mut stream, None, timeout).await? else {
                bail!("connection closed while awaiting challenge");
            };

            let their_challenge = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::Challenge { challenge },
                    ..
                } => {
                    if sequence_id != sequence_id_in {
                        bail!(
                            "unexpected sequence_id: expected {}, got {}",
                            sequence_id_in,
                            sequence_id
                        );
                    }
                    sequence_id_in += 1;
                    challenge
                }
                unexpected => bail!("expected Challenge, got {:?}", unexpected),
            };

            let mut payload = CHALLENGE_DOMAIN.to_vec();
            payload.extend_from_slice(&their_challenge);

            // Sign their challenge with a payload
            let our_signature = our_private_key.sign(&payload)?;

            debug!(%context_id, %our_identity, "Sending authentication response (initiator)");

            crate::sync::stream::send(
                &mut stream,
                &StreamMessage::Message {
                    sequence_id: sequence_id_out,
                    payload: MessagePayload::ChallengeResponse {
                        signature: our_signature.to_bytes(),
                    },
                    next_nonce: our_nonce,
                },
                None,
            )
            .await?;
            sequence_id_out += 1;

            // Step 5: Key exchange - initiator sends first
            crate::sync::stream::send(
                &mut stream,
                &StreamMessage::Message {
                    sequence_id: sequence_id_out,
                    payload: MessagePayload::KeyShare { sender_key },
                    next_nonce: our_nonce,
                },
                None,
            )
            .await?;

            // Receive their sender_key
            let Some(msg) = crate::sync::stream::recv(&mut stream, None, timeout).await? else {
                bail!("connection closed while awaiting key share");
            };

            let peer_sender_key = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::KeyShare { sender_key },
                    ..
                } => {
                    if sequence_id != sequence_id_in {
                        bail!(
                            "unexpected sequence_id: expected {}, got {}",
                            sequence_id_in,
                            sequence_id
                        );
                    }
                    sender_key
                }
                unexpected => bail!("expected KeyShare, got {:?}", unexpected),
            };

            their_identity_record.sender_key = Some(peer_sender_key);
        } else {
            // RESPONDER: Receive challenge first
            let Some(msg) = crate::sync::stream::recv(&mut stream, None, timeout).await? else {
                bail!("connection closed while awaiting challenge");
            };

            let their_challenge = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::Challenge { challenge },
                    ..
                } => {
                    if sequence_id != sequence_id_in {
                        bail!(
                            "unexpected sequence_id: expected {}, got {}",
                            sequence_id_in,
                            sequence_id
                        );
                    }
                    sequence_id_in += 1;
                    challenge
                }
                unexpected => bail!("expected Challenge, got {:?}", unexpected),
            };

            let mut payload = CHALLENGE_DOMAIN.to_vec();
            payload.extend_from_slice(&their_challenge);

            // Sign their challenge with a payload
            let our_signature = our_private_key.sign(&payload)?;

            debug!(%context_id, %our_identity, "Sending authentication response (responder)");

            crate::sync::stream::send(
                &mut stream,
                &StreamMessage::Message {
                    sequence_id: sequence_id_out,
                    payload: MessagePayload::ChallengeResponse {
                        signature: our_signature.to_bytes(),
                    },
                    next_nonce: our_nonce,
                },
                None,
            )
            .await?;
            sequence_id_out += 1;

            // Now send our challenge
            let challenge: [u8; 32] = rand::thread_rng().gen();

            debug!(%context_id, %their_identity, "Sending authentication challenge (responder)");

            crate::sync::stream::send(
                &mut stream,
                &StreamMessage::Message {
                    sequence_id: sequence_id_out,
                    payload: MessagePayload::Challenge { challenge },
                    next_nonce: our_nonce,
                },
                None,
            )
            .await?;
            sequence_id_out += 1;

            // Receive their signature
            let Some(msg) = crate::sync::stream::recv(&mut stream, None, timeout).await? else {
                bail!("connection closed while awaiting challenge response");
            };

            let their_signature_bytes = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::ChallengeResponse { signature },
                    ..
                } => {
                    if sequence_id != sequence_id_in {
                        bail!(
                            "unexpected sequence_id: expected {}, got {}",
                            sequence_id_in,
                            sequence_id
                        );
                    }
                    sequence_id_in += 1;
                    signature
                }
                unexpected => bail!("expected ChallengeResponse, got {:?}", unexpected),
            };

            let mut peer_payload = CHALLENGE_DOMAIN.to_vec();
            peer_payload.extend_from_slice(&challenge);

            // Verify their signature
            let their_signature = Signature::from_bytes(&their_signature_bytes);
            their_identity
                .verify(&peer_payload, &their_signature)
                .map_err(|e| eyre::eyre!("Peer failed to prove identity ownership: {}", e))?;

            debug!(%context_id, %their_identity, "Peer authenticated successfully");

            // Step 5: Key exchange - responder receives first
            let Some(msg) = crate::sync::stream::recv(&mut stream, None, timeout).await? else {
                bail!("connection closed while awaiting key share");
            };

            let peer_sender_key = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::KeyShare { sender_key },
                    ..
                } => {
                    if sequence_id != sequence_id_in {
                        bail!(
                            "unexpected sequence_id: expected {}, got {}",
                            sequence_id_in,
                            sequence_id
                        );
                    }
                    sender_key
                }
                unexpected => bail!("expected KeyShare, got {:?}", unexpected),
            };

            their_identity_record.sender_key = Some(peer_sender_key);

            // Then send our sender_key
            crate::sync::stream::send(
                &mut stream,
                &StreamMessage::Message {
                    sequence_id: sequence_id_out,
                    payload: MessagePayload::KeyShare { sender_key },
                    next_nonce: our_nonce,
                },
                None,
            )
            .await?;
        }

        // Step 6: Store their sender_key
        context_client.update_identity(context_id, &their_identity_record)?;

        info!(
            %context_id,
            %our_identity,
            their_identity=%their_identity_record.public_key,
            %peer,
            "Bidirectional key share completed with mutual authentication"
        );

        Ok(())
    })
    .await
    .map_err(|_| eyre::eyre!("Timeout during key share with peer"))?
}

/// Ensures the application blob is available for a context before handler execution.
///
/// This fixes the race condition where gossipsub state deltas arrive before the
/// WASM application blob has finished downloading. Without this check, handler
/// execution would fail with "ApplicationNotInstalled" errors.
///
/// The function polls for blob availability with exponential backoff, up to the
/// specified timeout. If the blob becomes available, it returns Ok(()); otherwise
/// it returns an error.
async fn ensure_application_available(
    node_client: &calimero_node_primitives::client::NodeClient,
    context_client: &calimero_context_primitives::client::ContextClient,
    context_id: &ContextId,
    timeout: std::time::Duration,
) -> Result<()> {
    use std::time::Duration;
    use tokio::time::{sleep, Instant};

    let context = context_client
        .get_context(context_id)?
        .ok_or_else(|| eyre::eyre!("context not found"))?;

    let application_id = context.application_id;

    // Check if application is already installed and blob available
    if let Ok(Some(app)) = node_client.get_application(&application_id) {
        // Application exists, check if bytecode blob is available
        if node_client.has_blob(&app.blob.bytecode)? {
            debug!(
                %context_id,
                %application_id,
                "Application blob already available"
            );
            return Ok(());
        }
    }

    // Blob not yet available - poll with backoff
    let start = Instant::now();
    let mut delay = Duration::from_millis(50);
    let max_delay = Duration::from_millis(500);

    info!(
        %context_id,
        %application_id,
        timeout_ms = timeout.as_millis(),
        "Waiting for application blob to become available..."
    );

    while start.elapsed() < timeout {
        sleep(delay).await;

        // Re-check application and blob
        if let Ok(Some(app)) = node_client.get_application(&application_id) {
            if node_client.has_blob(&app.blob.bytecode)? {
                info!(
                    %context_id,
                    %application_id,
                    elapsed_ms = start.elapsed().as_millis(),
                    "Application blob now available"
                );
                return Ok(());
            }
        }

        // Exponential backoff
        delay = std::cmp::min(delay * 2, max_delay);
    }

    // Timeout reached
    warn!(
        %context_id,
        %application_id,
        elapsed_ms = start.elapsed().as_millis(),
        "Timeout waiting for application blob"
    );

    Err(eyre::eyre!(
        "Application blob not available after {:?}",
        timeout
    ))
}
