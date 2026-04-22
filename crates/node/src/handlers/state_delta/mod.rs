//! State delta handling for BroadcastMessage::StateDelta
//!
//! **SRP**: This module has ONE job - process state deltas from peers using DAG

use calimero_context_client::client::ContextClient;
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
use crate::utils::choose_stream;

pub(crate) struct StateDeltaMessage {
    pub(crate) source: PeerId,
    pub(crate) context_id: ContextId,
    pub(crate) author_id: PublicKey,
    pub(crate) delta_id: [u8; 32],
    pub(crate) parent_ids: Vec<[u8; 32]>,
    pub(crate) hlc: calimero_storage::logical_clock::HybridTimestamp,
    pub(crate) root_hash: Hash,
    pub(crate) artifact: Vec<u8>,
    pub(crate) nonce: Nonce,
    pub(crate) events: Option<Vec<u8>>,
    pub(crate) governance_epoch: Vec<[u8; 32]>,
    pub(crate) key_id: [u8; 32],
}

pub(crate) struct StateDeltaContext {
    pub(crate) node_clients: crate::NodeClients,
    pub(crate) node_state: crate::NodeState,
    pub(crate) network_client: calimero_network_primitives::client::NetworkClient,
    pub(crate) sync_timeout: std::time::Duration,
}

pub(crate) struct ReplayBufferedDeltaInput {
    pub(crate) context_client: ContextClient,
    pub(crate) node_client: NodeClient,
    pub(crate) node_state: crate::NodeState,
    pub(crate) context_id: ContextId,
    pub(crate) our_identity: PublicKey,
    pub(crate) buffered: calimero_node_primitives::delta_buffer::BufferedDelta,
    pub(crate) sync_timeout: std::time::Duration,
    pub(crate) is_covered_by_checkpoint: bool,
}

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
    input: StateDeltaContext,
    message: StateDeltaMessage,
) -> Result<()> {
    let StateDeltaContext {
        node_clients,
        node_state,
        network_client,
        sync_timeout,
    } = input;
    let StateDeltaMessage {
        source,
        context_id,
        author_id,
        delta_id,
        parent_ids,
        hlc,
        root_hash,
        artifact,
        nonce,
        events,
        governance_epoch,
        key_id,
    } = message;

    let Some(context) = node_clients.context.get_context(&context_id)? else {
        bail!("context '{}' not found", context_id);
    };

    if calimero_context::group_store::is_read_only_for_context(
        node_clients.context.datastore(),
        &context_id,
        &author_id,
    )
    .unwrap_or(false)
    {
        warn!(
            %context_id,
            %author_id,
            "Rejecting state delta from ReadOnly member"
        );
        return Ok(());
    }

    info!(
        %context_id,
        %author_id,
        delta_id = ?delta_id,
        parent_count = parent_ids.len(),
        expected_root_hash = %root_hash,
        current_root_hash = %context.root_hash,
        governance_epoch_heads = governance_epoch.len(),
        "Received state delta"
    );

    // Governance catch-up is now handled by the namespace heartbeat
    // (NamespaceStateHeartbeat in network_event.rs). The governance_epoch
    // field on state deltas is retained for informational logging only.
    if !governance_epoch.is_empty() {
        tracing::debug!(
            %context_id,
            heads = governance_epoch.len(),
            "state delta carries governance epoch (catch-up via namespace heartbeat)"
        );
    }

    // Check if we should buffer this delta:
    // 1. During snapshot sync (sync session active)
    // 2. When context is uninitialized (can't decrypt without sender key)
    let is_uninitialized = context.root_hash == Hash::default();
    let should_buffer = node_state.should_buffer_delta(&context_id) || is_uninitialized;

    if should_buffer {
        info!(
            %context_id,
            delta_id = ?delta_id,
            is_uninitialized,
            has_events = events.is_some(),
            "Buffering delta (sync in progress or context uninitialized)"
        );

        let buffered = calimero_node_primitives::delta_buffer::BufferedDelta {
            id: delta_id,
            parents: parent_ids.clone(),
            hlc: hlc.get_time().as_u64(),
            payload: artifact.clone(),
            nonce,
            author_id,
            root_hash,
            events: events.clone(),
            source_peer: source,
            key_id,
        };

        if let Some(result) = node_state.buffer_delta(&context_id, buffered) {
            // Delta was handled by the buffer (added, evicted, or duplicate)
            // Only return early if it was successfully added or was a duplicate
            if result.was_added()
                || matches!(
                    result,
                    calimero_node_primitives::delta_buffer::PushResult::Duplicate
                )
            {
                return Ok(()); // Successfully buffered, will be replayed after sync
            }
            // If dropped due to zero capacity, fall through to normal processing
        }

        // No active session - if context is uninitialized, we must
        // start a session to buffer this delta
        if is_uninitialized && !node_state.should_buffer_delta(&context_id) {
            // Start a temporary buffer session for uninitialized context
            node_state.start_sync_session(context_id, hlc.get_time().as_u64());

            let buffered = calimero_node_primitives::delta_buffer::BufferedDelta {
                id: delta_id,
                parents: parent_ids.clone(),
                hlc: hlc.get_time().as_u64(),
                payload: artifact.clone(),
                nonce,
                author_id,
                root_hash,
                events: events.clone(),
                source_peer: source,
                key_id,
            };

            if let Some(result) = node_state.buffer_delta(&context_id, buffered) {
                if result.was_added()
                    || matches!(
                        result,
                        calimero_node_primitives::delta_buffer::PushResult::Duplicate
                    )
                {
                    info!(
                        %context_id,
                        delta_id = ?delta_id,
                        "Started buffer session for uninitialized context"
                    );
                    return Ok(());
                }
            }
        }

        warn!(
            %context_id,
            delta_id = ?delta_id,
            "Delta buffer full or zero capacity, proceeding with normal processing (may fail)"
        );
        // Fall through to normal processing
    }

    let group_key = {
        let store = node_clients.context.datastore();
        let gid = calimero_context::group_store::get_group_for_context(store, &context_id)?;
        match gid {
            Some(g) => calimero_context::group_store::load_group_key_by_id(store, &g, &key_id)?
                .map(PrivateKey::from)
                .ok_or_else(|| {
                    eyre::eyre!("no group key found for key_id {}", hex::encode(key_id))
                })?,
            None => {
                let identity = node_clients
                    .context
                    .get_identity(&context_id, &author_id)?
                    .ok_or_else(|| eyre::eyre!("no identity for author {author_id}"))?;
                identity
                    .sender_key
                    .ok_or_else(|| eyre::eyre!("no sender_key or group_key for context"))?
            }
        }
    };

    let actions = decrypt_delta_actions(artifact, nonce, group_key)?;

    let delta = calimero_dag::CausalDelta {
        id: delta_id,
        parents: parent_ids,
        payload: actions,
        hlc,
        expected_root_hash: *root_hash,
        kind: calimero_dag::DeltaKind::Regular,
    };

    let our_identity = choose_owned_identity(&node_clients.context, &context_id).await?;

    // Check if this is our own delta (gossipsub echoes back to sender).
    // Self-authored deltas are already applied locally, so we should NOT re-apply them.
    // This prevents state divergence from double-application of actions.
    let is_self_authored = author_id == our_identity;
    if is_self_authored {
        debug!(
            %context_id,
            %author_id,
            delta_id = ?delta_id,
            "Skipping self-authored delta (already applied locally)"
        );
        // Still emit events to WebSocket clients for consistency
        let events_payload = parse_events_payload(&events, &context_id);
        if let Some(payload) = events_payload {
            emit_state_mutation_event_parsed(&node_clients.node, &context_id, root_hash, payload)?;
        }
        return Ok(());
    }

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
            &delta_store_ref,
        )
        .await?;
        applied |= cascade_outcome.applied_current;
        handlers_already_executed |= cascade_outcome.handlers_executed_for_current;

        // Events-less deltas that the cascade applied to the DAG are not
        // present in `cascade_outcome.cascaded_events` (that collector only
        // surfaces deltas with persisted events to run handlers for), so
        // `applied_current` stays false even though the DAG state reflects
        // a successful apply. Check `missing_result.cascaded_ids` (the
        // full set of cascaded deltas produced by `get_missing_parents`,
        // including events-less ones) instead of re-acquiring the DAG
        // read lock via `dag_has_delta_applied`.
        if !applied && missing_result.cascaded_ids.contains(&delta_id) {
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

        if !missing_result.missing_ids.is_empty() {
            warn!(
                %context_id,
                missing_count = missing_result.missing_ids.len(),
                context_is_uninitialized = is_uninitialized,
                has_events = events.is_some(),
                "Delta pending due to missing parents - requesting them from peer"
            );

            match request_missing_deltas(
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
                Ok(peer_fetch_cascaded_events) => {
                    // Peer-fetched parents can cascade pending children via
                    // `apply_pending` inside `add_delta_with_events`. Those
                    // cascaded children's events were discarded before this
                    // fix — now they ride back on `peer_fetch_cascaded_events`
                    // and go through `execute_cascaded_events` exactly like
                    // the cascade path inside `get_missing_parents`.
                    if !peer_fetch_cascaded_events.is_empty() {
                        let cascade_outcome = execute_cascaded_events(
                            &peer_fetch_cascaded_events,
                            &node_clients,
                            &context_id,
                            &our_identity,
                            sync_timeout,
                            "peer-fetch cascade",
                            Some(&delta_id),
                            &delta_store_ref,
                        )
                        .await?;
                        applied |= cascade_outcome.applied_current;
                        handlers_already_executed |= cascade_outcome.handlers_executed_for_current;
                    }
                }
                Err(e) => {
                    warn!(?e, %context_id, ?source, "Failed to request missing deltas");
                }
            }

            // Some peer-fetched cascades may still apply the current delta
            // without having its events in the DB (events-less deltas are
            // never pre-persisted, so they won't show up in
            // `peer_fetch_cascaded_events`). The DAG state reflects the
            // apply regardless; check it before falling through to the
            // "still pending" path so we don't warn misleadingly.
            if !applied && delta_store_ref.dag_has_delta_applied(&delta_id).await {
                info!(
                    %context_id,
                    delta_id = ?delta_id,
                    "Delta was applied via cascade after peer-fetch of missing parents"
                );
                applied = true;
            }
        } else if !applied {
            // Parent is already in the database but `get_missing_parents`'s
            // explicit cascade didn't unblock this delta either. Rare, but
            // can happen if the DAG apply path itself returns an error for
            // the child. Left pending to retry on the next sync cycle.
            warn!(
                %context_id,
                delta_id = ?delta_id,
                has_events = events.is_some(),
                "Delta pending - parents exist but child did not apply during cascade"
            );
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
        &delta_store_ref,
    )
    .await?;

    // After successfully applying a remote delta, immediately broadcast our
    // updated root hash so lagging peers detect the divergence without waiting
    // for the 30-second periodic heartbeat.
    if applied {
        if let Ok(Some(ctx)) = node_clients.context.get_context(&context_id) {
            if !ctx.root_hash.is_zero() {
                let _ = node_clients
                    .node
                    .broadcast_heartbeat(&context_id, ctx.root_hash, ctx.dag_heads.clone())
                    .await;
            }
        }
    }

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
            // `load_persisted_deltas` surfaces any records with
            // `applied: true, events: Some(..)` — crash-leftovers
            // whose handlers never completed. Merged with the normal
            // cascade events below so a single handler pass covers both
            // (#2185). Share the DB scan with the DAG restore to avoid
            // a second full-table iteration (#2194 review).
            let pending_handler_events = match delta_store_ref.load_persisted_deltas().await {
                Ok(result) => {
                    if !result.pending_handler_events.is_empty() {
                        info!(
                            %context_id,
                            pending_count = result.pending_handler_events.len(),
                            "Replaying handlers interrupted by crash before events were cleared"
                        );
                    }
                    result.pending_handler_events
                }
                Err(e) => {
                    warn!(
                        ?e,
                        %context_id,
                        "Failed to load persisted deltas, starting with empty DAG"
                    );
                    Vec::new()
                }
            };

            let missing_result = delta_store_ref.get_missing_parents().await;
            if !missing_result.missing_ids.is_empty() {
                warn!(
                    %context_id,
                    missing_count = missing_result.missing_ids.len(),
                    "Missing parents after loading persisted deltas - will request from network"
                );
            }

            // The two sources are disjoint by construction:
            // `pending_handler_events` are records that were `applied:
            // true` on disk before this init ran, so they're restored
            // into the DAG as already-applied by `load_persisted_deltas`
            // and can't show up in `get_missing_parents`'s
            // pending→applied diff. Concat directly.
            let mut events_to_run = missing_result.cascaded_events;
            events_to_run.extend(pending_handler_events);

            execute_cascaded_events(
                &events_to_run,
                node_clients,
                &context_id,
                &our_identity,
                sync_timeout,
                "initial load",
                None,
                &delta_store_ref,
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
    delta_store: &DeltaStore,
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

                // Handlers ran successfully — clear events from the DB
                // record (#2185). If a later iteration's handler fails
                // with `?`, the remaining deltas keep `events: Some(..)`
                // and the next restart replays only those.
                delta_store.mark_events_executed(cascaded_id);

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
                    "Failed to deserialize cascaded events — clearing blob to prevent permanent replay loop"
                );
                // `serde_json::from_slice` failures on this blob are
                // structural, not transient: a blob that fails to
                // deserialize now will fail every restart. Without the
                // clear, `collect_pending_handler_events` would surface
                // this record on every init and we'd burn through the
                // same warn-and-skip cycle forever (#2194 review).
                delta_store.mark_events_executed(cascaded_id);
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
/// Fetch missing ancestor deltas from a peer and add them to the DAG in
/// topological order.
///
/// Returns the aggregated `cascaded_events` from every `add_delta_with_events`
/// call. Each peer-fetched parent that resolves a pending child carries that
/// child's stored events along in its `AddDeltaResult`; callers *must* run
/// `execute_cascaded_events` on the returned list, otherwise handler execution
/// for cascaded children silently never happens.
async fn request_missing_deltas(
    network_client: calimero_network_primitives::client::NetworkClient,
    sync_timeout: std::time::Duration,
    context_id: ContextId,
    missing_ids: Vec<[u8; 32]>,
    source: PeerId,
    our_identity: PublicKey,
    delta_store: DeltaStore,
) -> Result<Vec<([u8; 32], Vec<u8>)>> {
    use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

    // Open stream to peer
    let mut stream = network_client.open_stream(source).await?;

    // Fetch all missing ancestors, then add them in topological order (oldest first)
    let mut to_fetch = missing_ids;
    let mut fetched_deltas: Vec<(calimero_dag::CausalDelta<Vec<Action>>, [u8; 32])> = Vec::new();
    let mut fetch_count = 0;
    // Accumulated (delta_id, events_data) pairs from any cascades that
    // happen while adding peer-fetched parents below. Passed back to the
    // caller so handlers can run.
    let mut cascaded_events: Vec<([u8; 32], Vec<u8>)> = Vec::new();

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
                        kind: calimero_dag::DeltaKind::Regular,
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
            // Use the events-aware entry point so we can forward any events
            // attached to *cascaded children* to the caller. The peer-fetched
            // parent itself has no events (the wire protocol doesn't carry
            // them on `DeltaResponse`) — hence `None` for the second arg —
            // but `add_delta_internal`'s internal `apply_pending` can cascade
            // children that were pre-persisted with events, and those need
            // to reach `execute_cascaded_events` at the caller.
            match delta_store.add_delta_with_events(dag_delta, None).await {
                Ok(result) => {
                    if !result.cascaded_events.is_empty() {
                        info!(
                            %context_id,
                            parent_delta_id = ?delta_id,
                            cascaded_count = result.cascaded_events.len(),
                            "Peer-fetched parent cascaded pending children with events"
                        );
                        cascaded_events.extend(result.cascaded_events);
                    }
                }
                Err(e) => {
                    warn!(?e, %context_id, delta_id = ?delta_id, "Failed to add fetched delta to DAG");
                }
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

    Ok(cascaded_events)
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
    context_client: &calimero_context_client::client::ContextClient,
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

/// Replay a buffered delta after snapshot sync completes.
///
/// This function processes a delta that was buffered because the context was
/// uninitialized when it arrived. Now that the context is initialized (after
/// snapshot sync), we can decrypt it, apply it, and execute any event handlers.
///
/// The `is_covered_by_checkpoint` parameter indicates whether this delta is an
/// ancestor of an existing checkpoint. If true, the delta's state is already
/// present via the snapshot, and handlers should be executed even if the delta
/// can't be applied to the DAG (due to missing intermediate parents).
///
/// Returns Ok(true) if delta was applied, Ok(false) if pending (missing parents).
pub async fn replay_buffered_delta(input: ReplayBufferedDeltaInput) -> Result<bool> {
    let ReplayBufferedDeltaInput {
        context_client,
        node_client,
        node_state,
        context_id,
        our_identity,
        buffered,
        sync_timeout,
        is_covered_by_checkpoint,
    } = input;

    let delta_id = buffered.id;

    info!(
        %context_id,
        delta_id = ?delta_id,
        author = %buffered.author_id,
        has_events = buffered.events.is_some(),
        "Replaying buffered delta"
    );

    // Skip if this is our own delta
    if buffered.author_id == our_identity {
        debug!(
            %context_id,
            delta_id = ?delta_id,
            "Skipping replay of self-authored delta"
        );
        return Ok(false);
    }

    // Get context (should exist now after snapshot sync)
    let _context = context_client
        .get_context(&context_id)?
        .ok_or_else(|| eyre::eyre!("context not found after snapshot sync"))?;

    let group_key = {
        let store = context_client.datastore();
        let gid = calimero_context::group_store::get_group_for_context(store, &context_id)?;
        match gid {
            Some(g) => {
                calimero_context::group_store::load_group_key_by_id(store, &g, &buffered.key_id)?
                    .map(PrivateKey::from)
                    .ok_or_else(|| eyre::eyre!("no group key found for buffered delta"))?
            }
            None => {
                let identity = context_client
                    .get_identity(&context_id, &buffered.author_id)?
                    .ok_or_else(|| eyre::eyre!("no identity for buffered author"))?;
                identity
                    .sender_key
                    .ok_or_else(|| eyre::eyre!("no sender_key or group_key"))?
            }
        }
    };

    let actions = decrypt_delta_actions(buffered.payload, buffered.nonce, group_key)?;

    // Build the delta - reconstruct HLC from stored time
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use std::num::NonZeroU128;
    let default_id = ID::from(NonZeroU128::new(1).expect("1 is non-zero"));
    let hlc = HybridTimestamp::new(Timestamp::new(NTP64(buffered.hlc), default_id));

    let delta = calimero_dag::CausalDelta {
        id: buffered.id,
        parents: buffered.parents,
        payload: actions,
        hlc,
        expected_root_hash: *buffered.root_hash,
        kind: calimero_dag::DeltaKind::Regular,
    };

    // Get or create delta store - use [0u8; 32] as genesis hash placeholder
    // The actual genesis doesn't matter much for replay since the DAG already has
    // checkpoints from snapshot sync
    let delta_store = node_state
        .delta_stores
        .entry(context_id)
        .or_insert_with(|| {
            crate::delta_store::DeltaStore::new(
                [0u8; 32],
                context_client.clone(),
                context_id,
                our_identity,
            )
        })
        .clone();

    // Load any persisted deltas first
    let _ = delta_store.load_persisted_deltas().await;

    // If this delta is covered by checkpoint (ancestor of checkpoint) but is NOT the checkpoint
    // itself, skip adding it to the DAG. Its state is already present via snapshot, and adding
    // it would just put it in the pending queue forever (since its parents don't exist).
    let is_checkpoint_match = delta_store.dag_has_delta_applied(&delta_id).await;

    let add_result = if is_covered_by_checkpoint && !is_checkpoint_match {
        // Skip DAG addition for covered ancestor deltas
        // Return a "not applied" result since we're not adding to DAG
        debug!(
            %context_id,
            delta_id = ?delta_id,
            "Skipping DAG addition for ancestor delta (state covered by checkpoint)"
        );
        crate::delta_store::AddDeltaResult {
            applied: false,
            cascaded_events: vec![],
        }
    } else {
        // Normal case: add delta to DAG with events for handler execution
        delta_store
            .add_delta_with_events(delta.clone(), buffered.events.clone())
            .await?
    };

    // Re-check is_checkpoint_match after potential DAG add (for the case where we did add)
    let is_checkpoint_match =
        !add_result.applied && delta_store.dag_has_delta_applied(&delta_id).await;

    // Execute handlers if:
    // 1. Delta was applied (normal case), OR
    // 2. Delta matches a checkpoint (state exists via snapshot but handlers not yet run), OR
    // 3. Delta is covered by checkpoint (ancestor of checkpoint, state already in snapshot)
    //
    // Do NOT execute handlers if delta went to pending AND is NOT covered by checkpoint
    let should_execute_handlers =
        add_result.applied || is_checkpoint_match || is_covered_by_checkpoint;

    if should_execute_handlers {
        if let Some(events_data) = &buffered.events {
            let events_payload: Option<Vec<ExecutionEvent>> =
                match serde_json::from_slice(events_data) {
                    Ok(events) => Some(events),
                    Err(e) => {
                        warn!(
                            %context_id,
                            delta_id = ?delta_id,
                            error = %e,
                            "Failed to parse buffered events"
                        );
                        None
                    }
                };

            if let Some(events) = events_payload {
                // Check if we are the author (shouldn't be, but check anyway)
                let is_author = buffered.author_id == our_identity;
                if !is_author {
                    info!(
                        %context_id,
                        delta_id = ?delta_id,
                        events_count = events.len(),
                        applied_via_dag = add_result.applied,
                        is_checkpoint_match,
                        is_covered_by_checkpoint,
                        "Executing handlers for replayed buffered delta"
                    );

                    execute_event_handlers_parsed(
                        &context_client,
                        &context_id,
                        &our_identity,
                        &events,
                    )
                    .await?;
                }

                // Emit to WebSocket clients
                emit_state_mutation_event_parsed(
                    &node_client,
                    &context_id,
                    buffered.root_hash,
                    events,
                )?;
            }
        }
    } else {
        debug!(
            %context_id,
            delta_id = ?delta_id,
            has_events = buffered.events.is_some(),
            "Skipping handler execution for pending delta (will execute when delta is applied)"
        );
    }

    // Execute any cascaded handlers
    let node_clients = crate::NodeClients {
        context: context_client.clone(),
        node: node_client.clone(),
    };

    execute_cascaded_events(
        &add_result.cascaded_events,
        &node_clients,
        &context_id,
        &our_identity,
        sync_timeout,
        "buffered delta replay",
        None,
        &delta_store,
    )
    .await?;

    Ok(add_result.applied)
}
