//! Delta-store setup for state-delta handling: resolving the node's owned
//! identity for a context and constructing/locating the per-context
//! `DeltaStore` (including first-time initialization and crash recovery).
//!
//! Extracted from the state-delta handler; the apply path and the
//! buffered-delta replay path call these before applying.

use calimero_context_client::client::ContextClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result};
use tracing::{info, warn};

use crate::delta_store::DeltaStore;
use crate::utils::choose_stream;

use super::execute_cascaded_events;

pub(super) struct DeltaStoreSetup {
    pub(super) store: DeltaStore,
    pub(super) is_uninitialized: bool,
}

pub(super) async fn choose_owned_identity(
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

pub(super) async fn init_delta_store(
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
                &node_clients.node,
                &node_clients.context,
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
