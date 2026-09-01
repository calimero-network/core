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

use std::collections::HashMap;
use std::pin::pin;
use std::time::Duration;

use async_trait::async_trait;
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::{NamespaceJoinParams, OpenSubgroupJoinParams};
use calimero_node_primitives::join_bundle::JoinBundle;
use calimero_primitives::context::ContextId;
use eyre::Result;
use futures_util::stream::StreamExt;
use libp2p::PeerId;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{self, MissedTickBehavior};
use tracing::{debug, error, info, warn};

use super::session::{DispatchDecision, FullWarnHint, SessionTracker, SkipReason};
use crate::sync_session_bridge::{
    SyncSessionJob, SyncSessionResult, SyncSessionSendError, SyncSessionSender,
};

/// How many interval retries a namespace governance pull that delivered
/// nothing is owed before the driver stops re-arming it.
///
/// Sized for the gap it exists to cover — a link that is visible but not yet
/// usable — not for an absent peer. At the default interval that is a handful
/// of seconds, long enough for a connection to finish coming up and short
/// enough that a namespace with genuinely nothing to pull stops asking.
const MAX_NS_SYNC_RETRIES: u8 = 5;

/// Retries still owed to a namespace pull after an attempt that delivered
/// nothing, or `None` when the budget is spent and the driver should stop
/// re-arming it.
///
/// Its own function because the accounting is the whole safety property: too
/// eager and a namespace with nothing to pull becomes a permanent background
/// sync, too lax and the case this exists for — one lost attempt — is never
/// retried at all.
fn retries_left_after_failure(remaining: u8) -> Option<u8> {
    remaining.checked_sub(1).filter(|left| *left > 0)
}

/// Cross-actor message handlers and store accessors the driver calls
/// back into. Implemented by `SyncManager`; passed per-call to
/// [`SyncDriver::run`] for the same Send-safety + cycle-avoidance
/// reasons as `ReconcileSyncDispatch` and `ProtocolDispatch`.
#[async_trait(?Send)]
pub(crate) trait SyncDriverDispatch {
    /// Pull governance state for a namespace from a peer. Called from
    /// the `ns_sync_rx` arm.
    ///
    /// Returns the number of governance ops the pull delivered. Zero covers
    /// every best-effort failure — no peer, no stream, a peer with nothing to
    /// give — which the caller uses to decide whether the request still needs
    /// answering. See [`SyncDriver::run`]'s retry of an undelivered pull.
    async fn sync_namespace_from_peer(&self, namespace_id: [u8; 32]) -> usize;

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

    /// Run the sync-manager actor loop.
    ///
    /// Multiplexes over the six receivers, dispatches sync sessions
    /// for pending contexts, and drives the per-interval bookkeeping
    /// (full-drops rollup, wedge watchdog). The loop has no explicit
    /// termination condition — `next_sync.tick()` keeps firing even
    /// after every mpsc sender has been dropped, which matches the
    /// pre-extraction `SyncManager::start` behaviour. The actor is
    /// expected to live for the process's lifetime; shutdown happens
    /// by the process exiting rather than by graceful loop exit.
    pub(super) async fn run<D: SyncDriverDispatch>(mut self, dispatch: &D) {
        let mut next_sync = time::interval(self.frequency);
        next_sync.set_missed_tick_behavior(MissedTickBehavior::Delay);

        // Namespaces whose governance pull delivered nothing, with the number
        // of interval retries still owed to each.
        let mut pending_ns_sync: HashMap<[u8; 32], u8> = HashMap::new();

        loop {
            tokio::select! {
                _ = next_sync.tick() => {
                    debug!("Performing interval sync");

                    // Retry the governance pulls that delivered nothing. Doing
                    // it here rather than looping in place is what lets the
                    // reason they failed change — a connection finishing its
                    // handshake, a peer finishing its own join — instead of
                    // hammering the same unusable link.
                    for (namespace_id, remaining) in std::mem::take(&mut pending_ns_sync) {
                        if dispatch.sync_namespace_from_peer(namespace_id).await > 0 {
                            info!(
                                namespace_id = %hex::encode(namespace_id),
                                "namespace governance sync succeeded on retry"
                            );
                            continue;
                        }
                        if let Some(left) = retries_left_after_failure(remaining) {
                            let _ignored = pending_ns_sync.insert(namespace_id, left);
                        } else {
                            debug!(
                                namespace_id = %hex::encode(namespace_id),
                                "namespace governance sync still delivered nothing; \
                                 giving up until the next trigger"
                            );
                        }
                    }

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

                    // Periodic sweep: every context goes through the tracker's
                    // normal eligibility gate (force = false) and uses
                    // discovery-based peer selection.
                    self.dispatch_pending_contexts(HashMap::new(), true).await;
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
                    if dispatch.sync_namespace_from_peer(namespace_id).await == 0 {
                        // Nothing arrived, and nothing else will ask again.
                        //
                        // The request is an edge trigger with no periodic
                        // counterpart, so a pull that lands in the wrong
                        // moment is simply lost — and the moments are not
                        // rare. A node that rejoins a peer it was partitioned
                        // from is told to sync as soon as the peer is visible
                        // again, which is before the connection is usable, so
                        // the stream fails to open and the only request this
                        // node will ever make is spent. It then sits divergent
                        // with a healthy link, retrying nothing.
                        //
                        // Re-arm it on the interval instead. Bounded, because
                        // zero is also what a peer with genuinely nothing to
                        // give returns, and that must not become a permanent
                        // background pull.
                        let _ignored = pending_ns_sync.insert(namespace_id, MAX_NS_SYNC_RETRIES);
                    } else {
                        let _ignored = pending_ns_sync.remove(&namespace_id);
                    }
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
                    debug!(?ctx, ?peer, "Received sync request");

                    // Collect this request together with everything else
                    // already queued behind it, preserving each request's
                    // per-context peer hint. Multiple sync requests frequently
                    // enqueue near-simultaneously — most notably a burst of
                    // gossipsub `Subscribed` events on a context join, each
                    // requesting a targeted pull from the peer that just
                    // joined. Draining them lets every queued context be
                    // dispatched in this iteration (nothing is left stalled),
                    // while keeping the peer targeting and the
                    // explicit-request force-bypass that each request carries.
                    //
                    // Draining used to discard both: it counted the extra
                    // requests and then cleared the context and peer to force
                    // an all-contexts sweep, which defeated the one
                    // peer-targeted trigger (mesh-join pull) in exactly the
                    // high-contention burst it exists for, and downgraded
                    // operator resyncs to recency-gated no-ops.
                    //
                    // A bare `None` context is a global request and is the only
                    // thing that escalates to a full all-contexts sweep; a
                    // burst of purely context-specific requests dispatches
                    // exactly those contexts.
                    let mut explicit: HashMap<ContextId, Option<PeerId>> = HashMap::new();
                    let mut sweep_all = false;
                    ingest_sync_request(&mut explicit, &mut sweep_all, ctx, peer);

                    let mut drained_count = 0;
                    while let Ok((ctx, peer)) = self.ctx_sync_rx.try_recv() {
                        drained_count += 1;
                        ingest_sync_request(&mut explicit, &mut sweep_all, ctx, peer);
                    }
                    if drained_count > 0 {
                        debug!(
                            drained_count,
                            contexts = explicit.len(),
                            sweep_all,
                            "Coalesced additional queued sync requests"
                        );
                    }

                    self.dispatch_pending_contexts(explicit, sweep_all).await;
                }
            }
        }
    }

    /// Dispatch the sync requests assembled by the `ctx_sync_rx` arm or
    /// synthesised by the periodic `next_sync.tick()`.
    ///
    /// Two disjoint groups are dispatched:
    ///
    /// 1. **`explicit`** — contexts that were explicitly requested, each with
    ///    its (optional) peer hint. These are dispatched with `force = true`,
    ///    bypassing the dispatch-backoff and recency checks (but never
    ///    `AlreadyInProgress` — see `dispatch_decision`'s contract), and
    ///    forward their peer hint into `SyncSessionJob::Initiator`. Every
    ///    queued context is dispatched here, so no later-queued context is
    ///    starved.
    ///
    /// 2. **sweep** — when `sweep_all` is set (a global `sync(None, ..)`
    ///    request, or the periodic tick), every remaining context is walked
    ///    through the tracker's normal eligibility gate (`force = false`) with
    ///    discovery-based peer selection. Contexts already dispatched in group
    ///    1 are skipped, so a context named explicitly is not dispatched twice
    ///    in one iteration.
    async fn dispatch_pending_contexts(
        &mut self,
        explicit: HashMap<ContextId, Option<PeerId>>,
        sweep_all: bool,
    ) {
        // Group 1: explicit, forced, peer-targeted.
        for (&context_id, &peer_id) in &explicit {
            self.try_dispatch(context_id, peer_id, true).await;
        }

        // Group 2: full sweep, unforced, discovery-selected peer.
        if sweep_all {
            let contexts = self.context_client.get_context_ids(None);
            let mut contexts = pin!(contexts);

            while let Some(context_id) = contexts.next().await {
                let context_id = match context_id {
                    Ok(context_id) => context_id,
                    Err(err) => {
                        error!(%err, "Failed reading context id to sync");
                        continue;
                    }
                };

                if explicit.contains_key(&context_id) {
                    continue;
                }

                self.try_dispatch(context_id, None, false).await;
            }
        }
    }

    /// Attempt to dispatch a sync session for a single context.
    ///
    /// Consults the tracker for eligibility (`force` bypasses the
    /// dispatch-backoff and recency checks but not `AlreadyInProgress`),
    /// forwards a `SyncSessionJob::Initiator` carrying `peer_id` on success,
    /// and records the outcome (success / Full / Closed) back into the tracker.
    async fn try_dispatch(&mut self, context_id: ContextId, peer_id: Option<PeerId>, force: bool) {
        // Phase 1: read-only eligibility check. We must not mutate
        // state here because a failed `try_send` below would leave
        // `last_sync = None` with no future result to clear it —
        // permanently stalling the context (Cursor bugbot #2317).
        // The tracker rolls together the #2319 dispatch-attempt
        // backoff and the recency check; `force` (explicit
        // request) bypasses both.
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
                return;
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

        debug!(%context_id, "Scheduled sync");

        // Phase 2: dispatch BEFORE mutating state — so a
        // `Full`/`Closed` outcome leaves the per-context tracking
        // state untouched and the next interval tick (or
        // heartbeat trigger) just retries.
        let generation = self.tracker.begin_dispatch_generation(context_id);
        let dispatched = match self.session_tx.try_send(SyncSessionJob::Initiator {
            context_id,
            peer_id,
            generation,
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
            return;
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

/// Fold one `(context, peer)` sync request into the batch being assembled.
///
/// `Some(context)` records an explicit, peer-targeted request. A later request
/// naming a peer for a context that already has one wins, and a later request
/// with no peer never erases a hint already recorded — a targeted pull is the
/// more specific instruction, and losing it is the bug this whole path exists
/// to fix.
///
/// `None` is a global request and is the only thing that sets `sweep_all`.
fn ingest_sync_request(
    explicit: &mut HashMap<ContextId, Option<PeerId>>,
    sweep_all: &mut bool,
    ctx: Option<ContextId>,
    peer: Option<PeerId>,
) {
    match ctx {
        Some(context_id) => {
            let slot = explicit.entry(context_id).or_insert(peer);
            if peer.is_some() {
                *slot = peer;
            }
        }
        None => *sweep_all = true,
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

    // The request-coalescing logic (`ingest_sync_request`) has no such
    // constructor dependency and is unit-tested directly below — it is the part
    // that decides, for a burst of queued requests, which contexts get a forced
    // targeted dispatch and whether a full sweep is triggered.

    use calimero_primitives::context::ContextId;
    use libp2p::PeerId;

    use super::{ingest_sync_request, HashMap};

    fn ctx(byte: u8) -> ContextId {
        ContextId::from([byte; 32])
    }

    /// Fold a whole batch of requests, mirroring the `ctx_sync_rx` arm.
    fn ingest_all(
        requests: &[(Option<ContextId>, Option<PeerId>)],
    ) -> (HashMap<ContextId, Option<PeerId>>, bool) {
        let mut explicit = HashMap::new();
        let mut sweep_all = false;
        for &(c, p) in requests {
            ingest_sync_request(&mut explicit, &mut sweep_all, c, p);
        }
        (explicit, sweep_all)
    }

    #[test]
    fn single_targeted_request_preserves_peer_and_no_sweep() {
        let peer = PeerId::random();
        let (explicit, sweep_all) = ingest_all(&[(Some(ctx(1)), Some(peer))]);

        assert!(
            !sweep_all,
            "a context-specific request must not trigger a sweep"
        );
        assert_eq!(explicit.len(), 1);
        assert_eq!(explicit.get(&ctx(1)), Some(&Some(peer)));
    }

    #[test]
    fn burst_of_distinct_contexts_keeps_each_peer_and_no_sweep() {
        let (pa, pb, pc) = (PeerId::random(), PeerId::random(), PeerId::random());
        let (explicit, sweep_all) = ingest_all(&[
            (Some(ctx(1)), Some(pa)),
            (Some(ctx(2)), Some(pb)),
            (Some(ctx(3)), Some(pc)),
        ]);

        // Regression: a burst of purely context-specific requests must
        // dispatch exactly those contexts with their peers preserved —
        // not collapse into an untargeted all-contexts sweep.
        assert!(!sweep_all);
        assert_eq!(explicit.len(), 3);
        assert_eq!(explicit.get(&ctx(1)), Some(&Some(pa)));
        assert_eq!(explicit.get(&ctx(2)), Some(&Some(pb)));
        assert_eq!(explicit.get(&ctx(3)), Some(&Some(pc)));
    }

    #[test]
    fn last_targeted_peer_wins_for_same_context() {
        let (first, second) = (PeerId::random(), PeerId::random());
        let (explicit, sweep_all) =
            ingest_all(&[(Some(ctx(1)), Some(first)), (Some(ctx(1)), Some(second))]);

        assert!(!sweep_all);
        assert_eq!(explicit.len(), 1);
        assert_eq!(explicit.get(&ctx(1)), Some(&Some(second)));
    }

    #[test]
    fn targeted_hint_is_not_erased_by_later_untargeted_request() {
        let peer = PeerId::random();
        let (explicit, _) = ingest_all(&[(Some(ctx(1)), Some(peer)), (Some(ctx(1)), None)]);

        // An untargeted request for a context must not downgrade an
        // earlier targeted one to discovery-based selection.
        assert_eq!(explicit.get(&ctx(1)), Some(&Some(peer)));

        // ...and order-independent: untargeted first, targeted second.
        let (explicit, _) = ingest_all(&[(Some(ctx(2)), None), (Some(ctx(2)), Some(peer))]);
        assert_eq!(explicit.get(&ctx(2)), Some(&Some(peer)));
    }

    #[test]
    fn global_request_triggers_sweep() {
        let (explicit, sweep_all) = ingest_all(&[(None, None)]);

        assert!(sweep_all);
        assert!(explicit.is_empty());
    }

    #[test]
    fn mixed_batch_sweeps_and_keeps_explicit_targets() {
        let peer = PeerId::random();
        let (explicit, sweep_all) = ingest_all(&[
            (Some(ctx(1)), Some(peer)),
            (None, None),
            (Some(ctx(2)), None),
        ]);

        // A global request in the batch triggers the sweep, but the
        // explicitly-targeted contexts are still dispatched forced and
        // targeted (and skipped by the sweep via `contains_key`).
        assert!(sweep_all);
        assert_eq!(explicit.get(&ctx(1)), Some(&Some(peer)));
        assert_eq!(explicit.get(&ctx(2)), Some(&None));
    }

    use super::{retries_left_after_failure, MAX_NS_SYNC_RETRIES};

    /// A namespace pull that delivers nothing is retried, and the budget it is
    /// given is actually spent over several attempts rather than one.
    ///
    /// The first cut of this owed a single retry — enough to look right and to
    /// pass a test that only asserted "it retries", while still losing the case
    /// it exists for: a link visible but not yet usable, which needs a few
    /// seconds, not one tick.
    #[test]
    fn a_failed_namespace_pull_is_retried_until_its_budget_is_spent() {
        let mut remaining = MAX_NS_SYNC_RETRIES;
        let mut attempts = 0;
        while let Some(left) = retries_left_after_failure(remaining) {
            remaining = left;
            attempts += 1;
        }

        assert_eq!(
            attempts,
            MAX_NS_SYNC_RETRIES - 1,
            "the budget must be spent across attempts, not collapsed into one"
        );
        assert!(
            attempts > 1,
            "a single retry cannot cover a connection that is still coming up"
        );
    }

    /// And it does stop: zero is also what a peer with genuinely nothing to
    /// give returns, so an unbounded re-arm would turn every quiet namespace
    /// into a permanent background pull.
    #[test]
    fn a_spent_budget_stops_re_arming() {
        assert_eq!(retries_left_after_failure(1), None);
        assert_eq!(retries_left_after_failure(0), None);
    }
}
