//! State delta handling for BroadcastMessage::StateDelta
//!
//! **SRP**: This module has ONE job - process state deltas from peers using DAG

use calimero_crypto::Nonce;
use calimero_network_primitives::client::NetworkClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::ExecutionEvent;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt, Result};
use libp2p::PeerId;
use tracing::{info, warn};

use crate::delta_store::DeltaStore;
use crate::utils::choose_stream;

mod events;
mod key_share;
mod missing;

use events::{emit_state_mutation_event_parsed, execute_event_handlers_parsed};
use key_share::request_key_share_with_peer;
use missing::request_missing_deltas;

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
