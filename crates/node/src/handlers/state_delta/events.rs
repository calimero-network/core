//! Event execution for state-delta handling: running app event handlers,
//! cascading events from peer-fetched parent deltas, and emitting state
//! mutation events to WebSocket clients.
//!
//! Extracted from the state-delta handler; the orchestrators in `mod.rs`
//! call these after a delta's storage actions have been applied.

use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, NodeEvent, StateMutationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::Result;
use tracing::{debug, info, warn};

use crate::delta_store::DeltaStore;

use super::ensure_application_available;

// ---- CascadeOutcome ----
#[derive(Default)]
pub(super) struct CascadeOutcome {
    pub(super) applied_current: bool,
    pub(super) handlers_executed_for_current: bool,
}

// ---- execute_cascaded_events ----
/// Run the event handlers for a batch of cascaded deltas.
///
/// Error contract: every internal failure mode is deliberately downgraded to a
/// `warn!` and folded into `Ok(..)` — an unavailable application skips and
/// preserves the events for the next init, an undeserializable blob clears
/// itself to avoid a permanent replay loop, and a handler that errors leaves
/// its events in the DB (`mark_events_executed` is skipped) so the next restart
/// replays it at-least-once. The handler-failure policy is log-and-continue;
/// failures never unwind the caller. This function therefore returns `Ok` on
/// every path today. The `Result` is retained so a genuinely fatal future error
/// has somewhere to go, but callers must NOT use `?` to propagate it: doing so
/// would abort delta handling *after* the DAG has already been mutated. Match
/// and log instead.
pub(super) async fn execute_cascaded_events(
    cascaded_events: &[([u8; 32], Vec<u8>)],
    node_client: &NodeClient,
    context_client: &ContextClient,
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

    // `applied_current` tracks DAG-application state ONLY — whether the
    // current delta was among the cascaded set — and is deliberately set
    // before the app-availability check below. It does NOT imply handlers
    // ran (that is `handlers_executed_for_current`). When the app is
    // unavailable we return early with `applied_current = true` but
    // `handlers_executed_for_current = false`, so the caller's
    // `applied && !handlers_already_executed` guard still re-attempts
    // handler execution once the app is available. Callers MUST consult the
    // two flags separately; conflating them would skip handler replay.
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

    let app_available =
        ensure_application_available(node_client, context_client, context_id, sync_timeout)
            .await
            .is_ok();

    if !app_available {
        warn!(
            %context_id,
            cascaded_count = cascaded_events.len(),
            phase = phase,
            "Application not available - skipping cascaded handler execution. Events are preserved in DB (applied: true, events: Some(..)) and will replay on next init once the application becomes available."
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
                // Match-and-log rather than `?`: this function's contract
                // (see the doc comment) is log-and-continue, because a `?`
                // here would abort the cascade loop mid-batch *after* the
                // DAG was mutated, leaving later cascaded deltas
                // unprocessed. Treat a handler-execution error as "not all
                // succeeded" so the events blob is kept for restart replay.
                let all_succeeded = match execute_event_handlers_parsed(
                    context_client,
                    context_id,
                    our_identity,
                    &cascaded_payload,
                )
                .await
                {
                    Ok(succeeded) => succeeded,
                    Err(err) => {
                        warn!(
                            %context_id,
                            delta_id = ?cascaded_id,
                            error = %err,
                            phase = phase,
                            "Handler execution errored for cascaded delta; keeping events for restart replay"
                        );
                        false
                    }
                };

                // Clear the DB's `events` blob only when every handler
                // in the payload succeeded (#2185, #2194 review). On a
                // partial failure, leave `events: Some(..)` so the next
                // restart replays via `load_persisted_deltas`. Each
                // retry is at-least-once — handler idempotency concern
                // is tracked separately.
                if all_succeeded {
                    delta_store.mark_events_executed(cascaded_id);
                } else {
                    warn!(
                        %context_id,
                        delta_id = ?cascaded_id,
                        phase = phase,
                        "One or more handlers failed; keeping events in DB for restart replay"
                    );
                }

                if current_delta == Some(cascaded_id) {
                    // Handlers for the current delta were *attempted* —
                    // set this to `true` regardless of `all_succeeded`
                    // so `handle_state_delta`'s outer flow doesn't
                    // re-run them in the same request (which would
                    // duplicate the succeeded handlers). On partial
                    // failure, `mark_events_executed` above is skipped,
                    // so `events: Some(..)` stays in the DB and a
                    // restart replays — that is the retry path, not
                    // in-request re-execution.
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

// ---- execute_event_handlers_parsed ----
/// Execute event handlers for received events (from already-parsed payload)
///
/// # Handler Execution Model
///
/// **IMPORTANT**: Handlers currently execute **sequentially** in the order they appear
/// in the events array. Future optimization may execute handlers in **parallel**.
///
/// ## Requirements for Application Handlers
///
/// Event handlers **MUST** satisfy these properties to be correct:
///
/// 1. **Commutative**: Handler order must not affect final state
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
/// 4. **No side effects**: Handlers should only modify CRDT state
///    - ✅ SAFE: Pure state updates
///    - ❌ UNSAFE: HTTP requests, file I/O, blockchain transactions
///
/// ## Current Handler Implementations (Audited 2025-10-27)
///
/// All handlers in the codebase are **CRDT-only** operations:
/// - `kv-store-with-handlers`: All handlers just call `Counter::increment()`
/// - Other apps: No handlers defined
///
/// **Verdict**: Current handlers are **100% safe** for parallel execution.
///
/// ## Future Developers
///
/// If you're adding handlers that violate these assumptions:
/// 1. Document why parallelization is unsafe
/// 2. Consider refactoring to use CRDTs
/// 3. Or disable parallelization if absolutely necessary
///
/// Returns `Ok(true)` if every handler in the payload ran successfully,
/// `Ok(false)` if at least one handler errored (individual errors are
/// logged but swallowed so later handlers in the list still run). Callers
/// use the bool to decide whether it's safe to clear the persisted events
/// blob via `mark_events_executed` — clearing after a partial failure
/// would prevent restart-replay of the failed handlers (#2194 review).
pub(super) async fn execute_event_handlers_parsed(
    context_client: &ContextClient,
    context_id: &ContextId,
    our_identity: &PublicKey,
    events_payload: &[ExecutionEvent],
) -> Result<bool> {
    let mut all_succeeded = true;
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
                    all_succeeded = false;
                }
            }
        }
    }

    Ok(all_succeeded)
}

// ---- emit_state_mutation_event_parsed ----
/// Emit state mutation event to WebSocket clients (frontends)
///
/// Note: This is separate from node-to-node DAG synchronization.
/// - DAG broadcast (BroadcastMessage::StateDelta) = node-to-node sync
/// - WebSocket events (NodeEvent::Context) = node-to-frontend updates
///
/// Takes already-parsed events to avoid redundant deserialization
pub(super) fn emit_state_mutation_event_parsed(
    node_client: &NodeClient,
    context_id: &ContextId,
    root_hash: Hash,
    events_payload: Vec<ExecutionEvent>,
) {
    let state_mutation = ContextEvent {
        context_id: *context_id,
        payload: ContextEventPayload::StateMutation(StateMutationPayload::with_root_and_events(
            root_hash,
            events_payload,
        )),
    };

    // Infallible to callers: a failed WebSocket emit is logged and
    // swallowed (frontend notification is best-effort, not part of the
    // node-to-node apply path), so there is no error for callers to handle.
    if let Err(e) = node_client.send_event(NodeEvent::Context(state_mutation)) {
        warn!(
            %context_id,
            error = %e,
            "Failed to emit state mutation event to WebSocket clients"
        );
    }
}

// ---- parse_events_payload ----
/// Decode a delta's optional events blob into `ExecutionEvent`s.
///
/// Returns `None` both when there is no blob (`events == None`) and when the
/// blob is present but fails JSON deserialization (logged at `warn`). Callers
/// that need to distinguish the two — e.g. to clear a corrupt blob — check
/// `events.is_some()` alongside a `None` return.
pub(super) fn parse_events_payload(
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
