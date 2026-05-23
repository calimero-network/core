//! Sync-manager run-loop driver.
//!
//! Owns the actor-loop machinery that was previously inline in
//! `SyncManager::start`:
//!
//! - The six receive channels (`ctx_sync_rx`, `ns_sync_rx`,
//!   `ns_join_rx`, `open_subgroup_join_rx`, `session_result_rx`, plus
//!   the `next_sync` timer).
//! - The [`SyncSessionSender`] used to dispatch sync sessions.
//! - The [`SessionTracker`] (per-context state, dispatch backoff,
//!   wedge-watchdog, mailbox-full rollup).
//! - The per-interval dispatch loop that walks pending contexts and
//!   either forwards them into the session-actor or short-circuits
//!   via [`SessionTracker::dispatch_decision`].
//!
//! Extracted from `SyncManager::start` as Phase 5 of #2313. The
//! cross-actor message handlers (`sync_namespace_from_peer`,
//! `initiate_namespace_join`, `initiate_open_subgroup_join`) stay on
//! `SyncManager` and are exposed through the [`SyncDriverDispatch`]
//! trait, matching the per-call-injection pattern used by the
//! `Reconciler` and `ProtocolSelector` components.
//!
//! After this phase, `SyncManager::start` is a ~35-LOC shell that
//! takes the channel handles off `SyncManager`, constructs a
//! `SyncDriver`, and forwards `run(&self)`.

use std::pin::pin;
use std::time::Duration;

use async_trait::async_trait;
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::{NamespaceJoinParams, OpenSubgroupJoinParams};
use calimero_node_primitives::join_bundle::JoinBundle;
use calimero_primitives::context::ContextId;
use eyre::Result;
use futures_util::stream::{self, StreamExt};
use libp2p::PeerId;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{self, MissedTickBehavior};
use tracing::{debug, error, info, warn};

use super::session::{DispatchDecision, FullWarnHint, SessionTracker, SkipReason};
use crate::sync_session_bridge::{
    SyncSessionJob, SyncSessionResult, SyncSessionSendError, SyncSessionSender,
};

/// Cross-actor message handlers and store accessors the driver calls
/// back into. Implemented by `SyncManager`; passed per-call to
/// [`SyncDriver::run`] for the same Send-safety + cycle-avoidance
/// reasons as `ReconcileSyncDispatch` and `ProtocolDispatch`.
#[async_trait(?Send)]
pub(crate) trait SyncDriverDispatch {
    /// Pull governance state for a namespace from a peer. Called from
    /// the `ns_sync_rx` arm.
    async fn sync_namespace_from_peer(&self, namespace_id: [u8; 32]);

    /// Initiate the namespace-join handshake. Called from the
    /// `ns_join_rx` arm; the result is forwarded to the requester's
    /// `oneshot::Sender`.
    async fn initiate_namespace_join(&self, params: NamespaceJoinParams) -> Result<JoinBundle>;

    /// Initiate the open-subgroup-join handshake. Called from the
    /// `open_subgroup_join_rx` arm; the result is forwarded to the
    /// requester's `oneshot::Sender`.
    async fn initiate_open_subgroup_join(&self, params: OpenSubgroupJoinParams) -> Result<Vec<u8>>;
}

/// Sync-manager run-loop driver. Owned by `SyncManager::start` for
/// the lifetime of the actor.
pub(super) struct SyncDriver {
    tracker: SessionTracker,
    context_client: ContextClient,

    // Channel receivers, owned for the duration of the run loop.
    ctx_sync_rx: mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>,
    ns_sync_rx: mpsc::Receiver<[u8; 32]>,
    ns_join_rx: mpsc::Receiver<(NamespaceJoinParams, oneshot::Sender<Result<JoinBundle>>)>,
    open_subgroup_join_rx:
        mpsc::Receiver<(OpenSubgroupJoinParams, oneshot::Sender<Result<Vec<u8>>>)>,
    session_tx: SyncSessionSender,
    session_result_rx: mpsc::UnboundedReceiver<SyncSessionResult>,

    // Config derived from `SyncConfig`.
    frequency: Duration,
    interval: Duration,
}

impl SyncDriver {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        tracker: SessionTracker,
        context_client: ContextClient,
        ctx_sync_rx: mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>,
        ns_sync_rx: mpsc::Receiver<[u8; 32]>,
        ns_join_rx: mpsc::Receiver<(NamespaceJoinParams, oneshot::Sender<Result<JoinBundle>>)>,
        open_subgroup_join_rx: mpsc::Receiver<(
            OpenSubgroupJoinParams,
            oneshot::Sender<Result<Vec<u8>>>,
        )>,
        session_tx: SyncSessionSender,
        session_result_rx: mpsc::UnboundedReceiver<SyncSessionResult>,
        frequency: Duration,
        interval: Duration,
    ) -> Self {
        Self {
            tracker,
            context_client,
            ctx_sync_rx,
            ns_sync_rx,
            ns_join_rx,
            open_subgroup_join_rx,
            session_tx,
            session_result_rx,
            frequency,
            interval,
        }
    }

    /// Run the sync-manager actor loop until the input channels close.
    /// Multiplexes over the six receivers, dispatches sync sessions
    /// for pending contexts, and drives the per-interval bookkeeping
    /// (full-drops rollup, wedge watchdog).
    pub(super) async fn run<D: SyncDriverDispatch>(mut self, dispatch: &D) {
        let mut next_sync = time::interval(self.frequency);
        next_sync.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut requested_ctx = None;
        let mut requested_peer = None;

        loop {
            tokio::select! {
                _ = next_sync.tick() => {
                    debug!("Performing interval sync");
                    // #2319: roll up rate-limited mailbox-full drops.
                    if let Some(rollup) = self.tracker.tick_full_drops_summary() {
                        info!(
                            full_drops_in_window = rollup.drops,
                            contexts_affected = rollup.contexts_affected,
                            "SyncSession mailbox-full drop rollup (#2319)",
                        );
                    }
                    // #2319 watchdog: synthesise a failure for any
                    // context whose initiator hasn't produced a result
                    // within `session_wedge_grace`. The tracker applies
                    // `on_failure` on the returned contexts' state
                    // entries; we emit the per-context warn.
                    let grace = self.tracker.session_wedge_grace();
                    for context_id in self.tracker.tick_wedge_watchdog() {
                        warn!(
                            %context_id,
                            grace = ?grace,
                            "SyncSession initiator produced no result within watchdog grace — assuming a wedged session/actor; failing it so periodic-sync retries (#2319)"
                        );
                    }
                }
                Some(result) = self.session_result_rx.recv() => {
                    // `apply_result` clears the dispatch-attempt + wedge
                    // timers for the context AND updates `SyncState` —
                    // the per-arm logs are emitted from inside the
                    // tracker so the existing log shapes stay byte-
                    // identical to the pre-extraction text.
                    self.tracker.apply_result(result);
                    continue;
                }
                Some(namespace_id) = self.ns_sync_rx.recv() => {
                    info!(
                        namespace_id = %hex::encode(namespace_id),
                        "Performing namespace governance sync"
                    );
                    dispatch.sync_namespace_from_peer(namespace_id).await;
                    continue;
                }
                Some((params, reply_tx)) = self.ns_join_rx.recv() => {
                    info!(
                        namespace_id = %hex::encode(params.namespace_id),
                        "Processing namespace join request (initiator side)"
                    );
                    let result = dispatch.initiate_namespace_join(params).await;
                    let _ignored = reply_tx.send(result);
                    continue;
                }
                Some((params, reply_tx)) = self.open_subgroup_join_rx.recv() => {
                    info!(
                        namespace_id = %hex::encode(params.namespace_id),
                        subgroup_id = %hex::encode(params.subgroup_id),
                        "Processing open-subgroup join request (initiator side)"
                    );
                    let result = dispatch.initiate_open_subgroup_join(params).await;
                    let _ignored = reply_tx.send(result);
                    continue;
                }
                Some((ctx, peer)) = self.ctx_sync_rx.recv() => {
                    info!(?ctx, ?peer, "Received sync request");

                    requested_ctx = ctx;
                    requested_peer = peer;

                    // CRITICAL FIX: Drain all other pending sync requests in the queue.
                    // When multiple contexts join rapidly (common in E2E tests), they all
                    // call sync() which queues requests in ctx_sync_rx. The old code only
                    // processed ONE request per loop iteration, leaving contexts 2-N queued
                    // indefinitely. This caused those contexts to never sync and remain
                    // with dag_heads=[] and Uninitialized errors.
                    //
                    // Solution: Use try_recv() to drain all buffered requests immediately,
                    // then trigger a full sync that will process all contexts.
                    let mut drained_count = 0;
                    while self.ctx_sync_rx.try_recv().is_ok() {
                        drained_count += 1;
                    }

                    if drained_count > 0 {
                        info!(drained_count, "Drained additional sync requests from queue, will sync all contexts");
                        // Clear requested_ctx to force syncing ALL contexts
                        // This ensures newly-joined contexts get synced even if they weren't first in queue
                        requested_ctx = None;
                        requested_peer = None;
                    }
                }
            }

            self.dispatch_pending_contexts(requested_ctx.take(), requested_peer.take())
                .await;
        }
    }

    /// Walk pending contexts after a `next_sync.tick()` or
    /// `ctx_sync_rx` arm fired. For each context, consult the tracker
    /// for eligibility, attempt a `session_tx.try_send`, and record
    /// the outcome (success / Full / Closed) back into the tracker.
    ///
    /// `requested_ctx`/`requested_peer` mirror the explicit-request
    /// override the `ctx_sync_rx` arm captured: when present, `force`
    /// bypasses the dispatch-backoff and recency checks (but not
    /// `AlreadyInProgress` — see `dispatch_decision`'s contract).
    async fn dispatch_pending_contexts(
        &mut self,
        requested_ctx: Option<ContextId>,
        requested_peer: Option<PeerId>,
    ) {
        let contexts = requested_ctx
            .is_none()
            .then(|| self.context_client.get_context_ids(None));

        let contexts = stream::iter(requested_ctx)
            .map(Ok)
            .chain(stream::iter(contexts).flatten());

        let mut contexts = pin!(contexts);

        while let Some(context_id) = contexts.next().await {
            let context_id = match context_id {
                Ok(context_id) => context_id,
                Err(err) => {
                    error!(%err, "Failed reading context id to sync");
                    continue;
                }
            };

            // Phase 1: read-only eligibility check. We must not mutate
            // state here because a failed `try_send` below would leave
            // `last_sync = None` with no future result to clear it —
            // permanently stalling the context (Cursor bugbot #2317).
            // The tracker rolls together the #2319 dispatch-attempt
            // backoff and the recency check; `force` (explicit
            // request) bypasses both.
            let force = requested_ctx.is_some();
            let is_first_sync = match self.tracker.dispatch_decision(&context_id, force) {
                DispatchDecision::Skip(reason) => {
                    match reason {
                        SkipReason::DispatchRecentlyAttempted => debug!(
                            %context_id,
                            "Skipping sync — dispatch recently attempted, mailbox was full (#2319)"
                        ),
                        SkipReason::AlreadyInProgress => debug!(
                            %context_id,
                            "Sync already in progress"
                        ),
                        SkipReason::LastSyncTooRecent {
                            time_since,
                            minimum,
                        } => debug!(
                            %context_id,
                            ?time_since,
                            ?minimum,
                            "Skipping sync, last one was too recent"
                        ),
                    }
                    continue;
                }
                DispatchDecision::Eligible {
                    is_first_sync,
                    forced_despite_recency,
                } => {
                    if let Some(time_since) = forced_despite_recency {
                        debug!(
                            %context_id,
                            ?time_since,
                            minimum = ?self.interval,
                            "Force syncing despite recency, due to explicit request"
                        );
                    }
                    is_first_sync
                }
            };

            info!(%context_id, "Scheduled sync");

            // Phase 2: dispatch BEFORE mutating state — so a
            // `Full`/`Closed` outcome leaves the per-context tracking
            // state untouched and the next interval tick (or
            // heartbeat trigger) just retries.
            let dispatched = match self.session_tx.try_send(SyncSessionJob::Initiator {
                context_id,
                peer_id: requested_peer,
            }) {
                Ok(()) => true,
                Err(SyncSessionSendError::Full) => {
                    match self.tracker.record_dispatch_full(context_id) {
                        FullWarnHint::EmitWarn => warn!(
                            %context_id,
                            "SyncSession actor mailbox full — skipping initiator dispatch; backing off this context for {:?} (#2316/#2319)",
                            self.interval
                        ),
                        FullWarnHint::EmitDebug => debug!(
                            %context_id,
                            "SyncSession actor mailbox full — skipping (rate-limited; see periodic rollup) (#2319)"
                        ),
                    }
                    false
                }
                Err(SyncSessionSendError::Closed) => {
                    self.tracker.record_dispatch_closed(context_id);
                    warn!(
                        %context_id,
                        "SyncSession actor closed — skipping initiator dispatch"
                    );
                    false
                }
            };

            if !dispatched {
                continue;
            }

            // Phase 3: dispatch succeeded — mark the context as
            // in-flight. A `SyncSessionResult` will arrive on
            // `session_result_rx` and call `on_success` / `on_failure`
            // to clear the flag — or, if it never does, the #2319
            // watchdog above fails it after the grace.
            if is_first_sync {
                info!(%context_id, "Syncing for the first time");
            }
            self.tracker
                .record_dispatch_succeeded(context_id, is_first_sync);
        }
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    // Driver-level orchestration tests need a working `mpsc::Receiver`
    // pair AND a mockable `SyncDriverDispatch` AND a way to construct
    // the `SyncSessionSender` half. The first two are easy; the third
    // is currently tied to the `sync_session_bridge` actor wiring
    // and doesn't have a synthetic constructor. Tests will land in
    // a follow-up alongside the broader sync-test-fixture work
    // tracked in #2458 (which already enumerates the deferred test
    // sets for `Reconciler`, `SessionTracker`, and
    // `ProtocolSelector::execute`; adds `SyncDriver::run` to that
    // list).
    //
    // The dispatch-pending-contexts loop, the select! arm forwarders,
    // and the session-result apply path all move verbatim from
    // `SyncManager::start` — the existing partition-scenario
    // integration tests (`p3_dag_causal_tests`,
    // `p5_partition_scenarios_tests`) and the namespace-join /
    // open-subgroup-join e2e workflows continue to exercise the
    // driver's behaviour end-to-end in the meantime.
}
