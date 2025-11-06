//! State delta handling for BroadcastMessage::StateDelta
//!
//! **SRP**: This module has ONE job - process state deltas from peers using DAG

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
#[allow(clippy::too_many_arguments)]
pub async fn handle_state_delta(
    node_clients: crate::NodeClients,
    node_state: crate::NodeState,
    network_client: NetworkClient,
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

    // If we have the identity but missing sender_key, do direct key share with source peer
    if author_identity.sender_key.is_none() {
        info!(
            %context_id,
            %author_id,
            source_peer=%source,
            "Missing sender_key for author - initiating key share with source peer"
        );

        match request_key_share_with_peer(
            &network_client,
            &node_clients.context,
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
                author_identity = node_clients
                    .context
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
                DeltaStore::new(
                    [0u8; 32],
                    node_clients.context.clone(),
                    context_id,
                    our_identity,
                )
            });

        let delta_store_ref = delta_store.clone();
        (delta_store_ref, is_new)
    };

    if is_new_store {
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

    let add_result = delta_store_ref
        .add_delta_with_events(delta, events.clone())
        .await?;
    let mut applied = add_result.applied;
    let mut handlers_already_executed = false;

    if !applied {
        let missing_result = delta_store_ref.get_missing_parents().await;

        if !missing_result.cascaded_events.is_empty() {
            info!(
                %context_id,
                cascaded_count = missing_result.cascaded_events.len(),
                "Executing event handlers for deltas cascaded during missing parent check"
            );

            for (cascaded_id, events_data) in &missing_result.cascaded_events {
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

    if let Some(payload) = events_payload {
        emit_state_mutation_event_parsed(&node_clients.node, &context_id, root_hash, payload)?;
    }

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

async fn request_missing_deltas(
    network_client: NetworkClient,
    sync_timeout: std::time::Duration,
    context_id: ContextId,
    missing_ids: Vec<[u8; 32]>,
    source: PeerId,
    our_identity: PublicKey,
    delta_store: DeltaStore,
) -> Result<()> {
    use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

    let mut stream = network_client.open_stream(source).await?;

    let mut to_fetch = missing_ids;
    let mut fetched_deltas: Vec<(calimero_dag::CausalDelta<Vec<Action>>, [u8; 32])> = Vec::new();
    let mut fetch_count = 0;

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

            let timeout_budget = sync_timeout / 3;
            match crate::sync::stream::recv(&mut stream, None, timeout_budget).await? {
                Some(StreamMessage::Message {
                    payload: MessagePayload::DeltaResponse { delta },
                    ..
                }) => {
                    let storage_delta: calimero_storage::delta::CausalDelta =
                        borsh::from_slice(&delta)?;

                    info!(
                        %context_id,
                        delta_id = ?missing_id,
                        action_count = storage_delta.actions.len(),
                        "Received missing parent delta"
                    );

                    let dag_delta = calimero_dag::CausalDelta {
                        id: storage_delta.id,
                        parents: storage_delta.parents.clone(),
                        payload: storage_delta.actions,
                        hlc: storage_delta.hlc,
                        expected_root_hash: storage_delta.expected_root_hash,
                    };

                    fetched_deltas.push((dag_delta, missing_id));

                    for parent_id in &storage_delta.parents {
                        if *parent_id == [0; 32] {
                            continue;
                        }
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

    if !fetched_deltas.is_empty() {
        info!(
            %context_id,
            total_fetched = fetched_deltas.len(),
            "Adding fetched deltas to DAG in topological order"
        );

        fetched_deltas.reverse();

        for (dag_delta, delta_id) in fetched_deltas {
            if let Err(e) = delta_store.add_delta(dag_delta).await {
                warn!(?e, %context_id, delta_id = ?delta_id, "Failed to add fetched delta to DAG");
            }
        }

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

async fn request_key_share_with_peer(
    network_client: &NetworkClient,
    context_client: &ContextClient,
    context_id: &ContextId,
    author_identity: &PublicKey,
    peer: PeerId,
    timeout: std::time::Duration,
) -> Result<()> {
    use calimero_crypto::{Nonce, SharedKey};
    use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
    use rand::Rng;

    debug!(
        %context_id,
        %author_identity,
        %peer,
        "Initiating bidirectional key share with peer"
    );

    tokio::time::timeout(timeout, async {
        let mut stream = network_client.open_stream(peer).await?;

        let identities = context_client.get_context_members(context_id, Some(true));
        let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context");
        };

        let our_nonce = rand::thread_rng().gen::<Nonce>();

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

        let Some(ack) = crate::sync::stream::recv(&mut stream, None, timeout).await? else {
            bail!("connection closed while awaiting key share handshake");
        };

        let their_nonce = match ack {
            StreamMessage::Init {
                payload: InitPayload::KeyShare,
                next_nonce,
                ..
            } => next_nonce,
            unexpected => {
                bail!("unexpected message during key share: {:?}", unexpected)
            }
        };

        let mut their_identity = context_client
            .get_identity(context_id, author_identity)?
            .ok_or_eyre("expected peer identity to exist")?;

        let (private_key, sender_key) = context_client
            .get_identity(context_id, &our_identity)?
            .and_then(|i| Some((i.private_key?, i.sender_key?)))
            .ok_or_eyre("expected own identity to have private & sender keys")?;

        let shared_key = SharedKey::new(&private_key, &their_identity.public_key);

        crate::sync::stream::send(
            &mut stream,
            &StreamMessage::Message {
                sequence_id: 0,
                payload: MessagePayload::KeyShare { sender_key },
                next_nonce: our_nonce,
            },
            Some((shared_key, our_nonce)),
        )
        .await?;

        let Some(msg) =
            crate::sync::stream::recv(&mut stream, Some((shared_key, their_nonce)), timeout)
                .await?
        else {
            bail!("connection closed while awaiting sender_key");
        };

        let their_sender_key = match msg {
            StreamMessage::Message {
                payload: MessagePayload::KeyShare { sender_key },
                ..
            } => sender_key,
            unexpected => {
                bail!("unexpected message: {:?}", unexpected)
            }
        };

        their_identity.sender_key = Some(their_sender_key);
        context_client.update_identity(context_id, &their_identity)?;

        info!(
            %context_id,
            our_identity=%our_identity,
            their_identity=%author_identity,
            %peer,
            "Bidirectional key share completed"
        );

        Ok(())
    })
    .await
    .map_err(|_| eyre::eyre!("Timeout during key share with peer"))??
}
