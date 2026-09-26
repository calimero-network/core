//! Event execution for state-delta handling: running app event handlers,
//! cascading events from peer-fetched parent deltas, and emitting state
//! mutation events to WebSocket clients.
//!
//! Extracted from the state-delta handler; the orchestrators in `mod.rs`
//! call these after a delta's storage actions have been applied.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use calimero_context_client::client::ContextClient;
use calimero_context_client::tee_trigger;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, NodeEvent, StateMutationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::logical_clock;
use calimero_store::key::ContextDagDelta as ContextDagDeltaKey;
use eyre::Result;
use tracing::{debug, info, warn};

use crate::delta_store::DeltaStore;

use super::{application_bytecode_status, BytecodeStatus};

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
#[allow(
    clippy::too_many_arguments,
    reason = "orthogonal args on a consensus sync/handler path; no cohesive grouping"
)]
pub(super) async fn execute_cascaded_events(
    cascaded_events: &[([u8; 32], Vec<u8>)],
    node_client: &NodeClient,
    context_client: &ContextClient,
    context_id: &ContextId,
    our_identity: &PublicKey,
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

    let app_available = matches!(
        application_bytecode_status(node_client, context_client, context_id),
        Ok(BytecodeStatus::Ready)
    );

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
                    cascaded_id,
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
                        "One or more handlers failed or wait for their turn; keeping events in DB for restart replay"
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
/// ## TEE handlers
///
/// A handler named `tee:<method>` is a **TEE trigger**: it runs on one node, a
/// TEE authority, through [`ContextClient::execute_tee_trigger`], as
/// `AccountId::TEE_AUTHORITY`. The authorities are ranked per delta
/// ([`tee_rank_for`]); the first fires at once and each later one waits its
/// turn and fires only if no firing has reached it ([`plan_tee_firing`]). A
/// node that is no authority skips the handler, and a skip counts as success:
/// it is not this node's to run, so there is nothing to replay on restart.
///
/// A waiting authority returns `Ok(false)` so the events stay in the DB: a
/// restart before its turn comes replays them, and the replay finds the
/// trigger fired, or waits again.
///
/// Returns `Ok(true)` if every handler in the payload ran successfully,
/// `Ok(false)` if at least one handler errored or waits for its turn
/// (individual errors are logged but swallowed so later handlers in the list
/// still run). Callers use the bool to decide whether it's safe to clear the
/// persisted events blob via `mark_events_executed` — clearing after a partial
/// failure would prevent restart-replay of the failed handlers (#2194 review).
pub(super) async fn execute_event_handlers_parsed(
    context_client: &ContextClient,
    context_id: &ContextId,
    our_identity: &PublicKey,
    // The delta carrying these events. It names the firing, so it is what the
    // TEE election ranks on.
    cause: &[u8; 32],
    events_payload: &[ExecutionEvent],
) -> Result<bool> {
    record_fired_markers(context_client, context_id, cause, events_payload);

    let mut all_succeeded = true;
    // Resolved once per delta, and only if it carries a TEE handler.
    let mut rank: Option<Option<usize>> = None;
    for event in events_payload {
        if let Some(tee_method) = event
            .handler
            .as_deref()
            .and_then(|handler| handler.strip_prefix(TEE_HANDLER_PREFIX))
        {
            let our_rank = match rank {
                Some(our_rank) => our_rank,
                None => match tee_rank_for(context_client, context_id, our_identity, cause) {
                    Ok(our_rank) => {
                        rank = Some(our_rank);
                        our_rank
                    }
                    // Not a verdict, so neither "skip" nor abort: keep the
                    // events for replay and let the other handlers run.
                    Err(err) => {
                        warn!(%context_id, tee_method, error = %err, "TEE election lookup failed");
                        all_succeeded = false;
                        continue;
                    }
                },
            };
            let firing = TeeFiring {
                context_id: *context_id,
                executor: *our_identity,
                method: tee_method.to_owned(),
                payload: event.data.clone(),
                trigger: tee_trigger::event_trigger_id(cause, tee_method),
            };
            match firing.plan(context_client, our_rank, cause) {
                Ok(TeePlan::NotOurs) => {
                    debug!(
                        %context_id,
                        tee_method,
                        "Skipping TEE handler: this node is not a TEE authority"
                    );
                }
                Ok(TeePlan::AlreadyFired) => {
                    debug!(%context_id, tee_method, "Skipping TEE handler: already fired");
                }
                Ok(TeePlan::Now) => {
                    if !firing.fire(context_client).await {
                        all_succeeded = false;
                    }
                }
                Ok(TeePlan::After(delay)) => {
                    firing.fire_after(context_client.clone(), delay);
                    all_succeeded = false;
                }
                Err(err) => {
                    warn!(%context_id, tee_method, error = %err, "TEE firing lookup failed");
                    all_succeeded = false;
                }
            }
            continue;
        }
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

/// Handler-name prefix marking an event handler as a TEE trigger.
const TEE_HANDLER_PREFIX: &str = "tee:";

/// Domain separator for ranking TEE authorities per firing.
const TEE_TRIGGER_RANK_DOMAIN: &[u8] = b"calimero.tee-trigger-rank.v1";

/// How long each TEE authority's turn lasts. The authority ranked `k` fires
/// `k` turns after the delta arrives, if no firing has reached it by then.
///
/// Long enough for a firing to gossip to every other authority, so a live
/// first-ranked TEE is not doubled by the second; short enough that a game
/// whose TEE is down stalls for seconds, not minutes.
pub(crate) const TEE_FAILOVER_GRACE: Duration = Duration::from_secs(15);

/// Record the triggers this delta says it fired, when a TEE authority signed
/// it.
///
/// Best-effort: a marker that is not recorded costs at most a duplicate firing
/// by a fallback TEE, and failing the delta over it would be worse.
fn record_fired_markers(
    context_client: &ContextClient,
    context_id: &ContextId,
    delta_id: &[u8; 32],
    events_payload: &[ExecutionEvent],
) {
    let mut markers = tee_trigger::fired_markers(
        events_payload
            .iter()
            .map(|event| (event.kind.as_str(), event.data.as_slice())),
    )
    .peekable();
    if markers.peek().is_none() {
        return;
    }
    let store = context_client.datastore();
    let author = match store
        .handle()
        .get(&ContextDagDeltaKey::new(*context_id, *delta_id))
    {
        Ok(Some(row)) => row.author_id,
        Ok(None) => None,
        Err(err) => {
            warn!(%context_id, error = %err, "Cannot read a delta's author to honour its TEE markers");
            return;
        }
    };
    // Anyone can emit an event of the marker's kind. Only one from a delta a
    // TEE authority signed says a trigger fired; a member's would let them
    // stall a game whose elected TEE is down.
    let signed_by_authority = author.is_some_and(|author| {
        calimero_governance_store::is_attested_tee_key_for_context(store, context_id, &author)
            .unwrap_or_else(|err| {
                warn!(%context_id, error = %err, "TEE authority lookup failed for a fired marker");
                false
            })
    });
    if !signed_by_authority {
        debug!(%context_id, "Ignoring TEE fired markers on a delta no TEE authority signed");
        return;
    }
    for trigger in markers {
        if let Err(err) = tee_trigger::record_tee_fired(store, context_id, &trigger) {
            warn!(%context_id, error = %err, "Failed to record a TEE fired marker");
        }
    }
}

/// This node's place in the order the TEE authorities fire the TEE handlers of
/// the delta `cause`, or `None` if it is not a TEE authority for the context.
///
/// Every authority ranks all of them by `H(cause ‖ account)`, lowest first.
/// Ranking on the delta rather than on anything a TEE produces means no TEE can
/// grind an outcome by choosing whether to fire, and needs no messages.
fn tee_rank_for(
    context_client: &ContextClient,
    context_id: &ContextId,
    our_identity: &PublicKey,
    cause: &[u8; 32],
) -> Result<Option<usize>> {
    let store = context_client.datastore();
    if !calimero_governance_store::is_tee_authority_for_context(store, context_id, our_identity)? {
        return Ok(None);
    }
    let Some(group_id) = calimero_governance_store::get_group_for_context(store, context_id)?
    else {
        return Ok(None);
    };
    let Some(our_account) =
        calimero_governance_store::member_account_in_namespace(store, &group_id, our_identity)?
    else {
        return Ok(None);
    };
    let mut ranked = calimero_governance_store::tee_authorities_for_context(store, context_id)?;
    ranked.sort_by_cached_key(|account| {
        calimero_primitives::identity::domain_hash(
            TEE_TRIGGER_RANK_DOMAIN,
            &[cause.as_slice(), account.as_bytes().as_slice()],
        )
    });
    Ok(ranked.iter().position(|account| *account == our_account))
}

/// What a TEE authority does with one TEE handler.
#[derive(Debug, PartialEq, Eq)]
enum TeePlan {
    /// This node is no TEE authority for the context.
    NotOurs,
    /// Some authority fired it already.
    AlreadyFired,
    /// Fire it now.
    Now,
    /// Fire it after this long, unless a firing arrives first.
    After(Duration),
}

/// When the authority ranked `rank` fires a trigger that has not fired yet.
///
/// The first-ranked fires at once, but only on a delta that is **fresh**: one
/// whose clock is within a turn of this node's. A stale delta is one this node
/// is catching up on, after being down or partitioned, and a fallback may well
/// have fired it meanwhile; that firing is still on its way, so even the
/// first-ranked waits a turn for it. Every later rank waits one more turn than
/// the rank before. `cause_age` is `None` when the delta's clock is unknown,
/// which counts as stale.
fn plan_tee_firing(rank: usize, cause_age: Option<Duration>, grace: Duration) -> TeePlan {
    let fresh = cause_age.is_some_and(|age| age <= grace);
    let turns = rank.saturating_add(usize::from(!fresh));
    match u32::try_from(turns) {
        Ok(0) => TeePlan::Now,
        Ok(turns) => TeePlan::After(grace.saturating_mul(turns)),
        Err(_) => TeePlan::After(Duration::MAX),
    }
}

/// Triggers a fallback is waiting on in this process, so a delta replayed
/// while it waits does not start a second wait.
static WAITING: LazyLock<Mutex<HashSet<WaitingKey>>> = LazyLock::new(Default::default);

/// A context and one of its triggers.
type WaitingKey = ([u8; 32], tee_trigger::TeeTriggerId);

fn waiting() -> std::sync::MutexGuard<'static, HashSet<WaitingKey>> {
    WAITING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One TEE handler this node may fire.
struct TeeFiring {
    context_id: ContextId,
    executor: PublicKey,
    method: String,
    payload: Vec<u8>,
    trigger: tee_trigger::TeeTriggerId,
}

impl TeeFiring {
    fn plan(
        &self,
        context_client: &ContextClient,
        rank: Option<usize>,
        cause: &[u8; 32],
    ) -> Result<TeePlan> {
        let Some(rank) = rank else {
            return Ok(TeePlan::NotOurs);
        };
        let store = context_client.datastore();
        if tee_trigger::tee_fired(store, &self.context_id, &self.trigger)? {
            return Ok(TeePlan::AlreadyFired);
        }
        let cause_age = store
            .handle()
            .get(&ContextDagDeltaKey::new(self.context_id, *cause))?
            .map(|row| {
                let sent = UNIX_EPOCH
                    + Duration::from_secs(u64::from(logical_clock::physical_time_secs(&row.hlc)));
                // A clock ahead of ours is as fresh as a delta can be.
                SystemTime::now().duration_since(sent).unwrap_or_default()
            });
        Ok(plan_tee_firing(rank, cause_age, TEE_FAILOVER_GRACE))
    }

    /// Fire now. `true` if the run went through.
    async fn fire(&self, context_client: &ContextClient) -> bool {
        let context_id = &self.context_id;
        let tee_method = &self.method;
        info!(%context_id, tee_method, "Firing TEE trigger");
        match context_client
            .execute_tee_trigger(
                context_id,
                &self.executor,
                self.method.clone(),
                self.payload.clone(),
                self.trigger,
            )
            .await
        {
            Ok(_) => {
                // Our own firing never comes back to us as a received delta.
                if let Err(err) = tee_trigger::record_tee_fired(
                    context_client.datastore(),
                    context_id,
                    &self.trigger,
                ) {
                    warn!(%context_id, tee_method, error = %err, "Failed to record our own TEE firing");
                }
                true
            }
            Err(err) => {
                warn!(tee_method, error = %err, "TEE trigger failed");
                false
            }
        }
    }

    /// Wait `delay`, then fire unless a firing has arrived meanwhile.
    ///
    /// Not persisted: the caller keeps the delta's events in the DB, so a
    /// restart before the turn comes replays them and waits again.
    fn fire_after(self, context_client: ContextClient, delay: Duration) {
        let key = (*self.context_id, self.trigger);
        if !waiting().insert(key) {
            return;
        }
        let context_id = self.context_id;
        let tee_method = self.method.clone();
        info!(%context_id, tee_method, ?delay, "Waiting to fall back on a TEE trigger");
        drop(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            match tee_trigger::tee_fired(context_client.datastore(), &context_id, &self.trigger) {
                Ok(true) => {
                    debug!(%context_id, tee_method, "TEE trigger fired elsewhere; standing down");
                }
                Ok(false) => {
                    info!(%context_id, tee_method, "Falling back on a TEE trigger");
                    let _ = self.fire(&context_client).await;
                }
                Err(err) => {
                    warn!(%context_id, tee_method, error = %err, "TEE fired lookup failed; not falling back");
                }
            }
            let _ = waiting().remove(&key);
        }));
    }
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
    mut events_payload: Vec<ExecutionEvent>,
) {
    // The TEE fired marker is node-to-node bookkeeping, not an app event.
    events_payload.retain(|event| event.kind != tee_trigger::TEE_FIRED_EVENT_KIND);
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

#[cfg(test)]
mod tee_failover_tests {
    use super::*;

    const GRACE: Duration = Duration::from_secs(10);

    #[test]
    fn the_first_ranked_fires_a_fresh_trigger_at_once() {
        assert_eq!(
            plan_tee_firing(0, Some(Duration::ZERO), GRACE),
            TeePlan::Now
        );
        assert_eq!(plan_tee_firing(0, Some(GRACE), GRACE), TeePlan::Now);
    }

    #[test]
    fn each_later_rank_waits_one_more_turn() {
        let fresh = Some(Duration::from_secs(1));
        assert_eq!(plan_tee_firing(1, fresh, GRACE), TeePlan::After(GRACE));
        assert_eq!(plan_tee_firing(3, fresh, GRACE), TeePlan::After(GRACE * 3));
    }

    /// A node catching up must not fire before the fallback's firing it has
    /// not received yet, so a stale trigger costs every rank one extra turn.
    #[test]
    fn a_stale_or_undated_trigger_waits_a_turn_even_for_the_first_ranked() {
        let stale = Some(GRACE + Duration::from_secs(1));
        assert_eq!(plan_tee_firing(0, stale, GRACE), TeePlan::After(GRACE));
        assert_eq!(plan_tee_firing(2, stale, GRACE), TeePlan::After(GRACE * 3));
        assert_eq!(plan_tee_firing(0, None, GRACE), TeePlan::After(GRACE));
    }
}
