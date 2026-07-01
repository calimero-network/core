//! Sync-session dispatch actor.
//!
//! Moves HashComparison/LevelWise sync sessions (both initiator and
//! responder sides) off the `NodeManager` arbiter and the
//! `SyncManager::start` select loop onto a dedicated `SyncSessionActor`
//! running on its own Arbiter (issue #2316, follow-up to #2299/#2293).
//!
//! ## Why this exists
//!
//! Pre-#2316 the responder ran on the `NodeManager` arbiter
//! (via `ctx.spawn` in `handlers/stream_opened.rs`) and the initiator
//! ran inline inside `SyncManager::start`'s `FuturesUnordered`. A
//! single slow session (#2199 makes 5–10s sessions plausible under
//! fuzzy load) blocked the same task that drives gossipsub
//! `Swarm::poll`, draining the libp2p stream-accept channel and
//! letting mesh peers prune the busy node — exactly the failure
//! described in #2293.
//!
//! ## Backpressure (#2316 + #2319)
//!
//! Bounded Actix mailbox via `set_mailbox_capacity`; `Addr::try_send`
//! returns `SendError::Full` on overflow. On overflow the dispatch
//! site backs the context off for one sync interval (#2319 — see
//! `SyncManager::start`) instead of re-attempting every tick; the
//! periodic-sync interval and heartbeat-driven sync triggers cover
//! dropped initiators, and peers will retry dropped responder streams
//! via their own retry logic. Drops are counted per-reason in
//! [`SyncSessionMetrics`].
//!
//! In addition (#2319) a per-`ContextId` in-flight gate refuses a
//! second initiator for a context that already has one running, and
//! each session runs under `sync_config.session_deadline` — an outer
//! `tokio::time::timeout` (defaults to the 30 s `sync_config.timeout`,
//! the per-step budget; lowerable per deployment) so a stuck session
//! frees its slot — and stops burning the actor's single arbiter
//! thread — predictably.
//!
//! ## Mirrors `state_delta_bridge`
//!
//! Same shape as `state_delta_bridge::StateDeltaActor`: dedicated
//! Arbiter, bounded mailbox, `try_send`, `InFlightGuard`,
//! per-session `tokio::time::timeout`, and counters for
//! processed/error/timeout/dropped logged once a minute.

use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use actix::{
    Actor, ActorFutureExt, Addr, ArbiterHandle, AsyncContext, Context, Handler, Message, WrapFuture,
};
use calimero_network_primitives::stream::Stream;
use calimero_primitives::context::ContextId;
use dashmap::DashMap;
use libp2p::PeerId;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::registry::Registry;
use tokio::sync::{mpsc, Semaphore};
use tracing::{debug, info, warn};

use calimero_node_primitives::sync::SyncProtocol;

use crate::sync::SyncManager;

/// Mailbox capacity for the sync-session actor.
///
/// Sync sessions are heavier than state-delta jobs but much less
/// frequent (a few per second per context vs. many per second). 256
/// covers >30s of bursts at the typical rate before drops; on
/// overflow we drop and rely on the periodic-sync interval to retry.
pub const SYNC_SESSION_CHANNEL_CAPACITY: usize = 256;

/// Periodic summary log interval.
const SUMMARY_INTERVAL: Duration = Duration::from_secs(60);

/// Why a [`SyncSessionJob`] was dropped instead of run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropReason {
    /// Actor mailbox at capacity (`try_send` -> `Full`).
    MailboxFull,
    /// Actor stopped / shutting down (`try_send` -> `Closed`).
    ActorClosed,
    /// An `Initiator` job arrived for a `ContextId` that already has a
    /// session in flight on the actor (#2319 per-context gate).
    ContextBusy,
}

impl DropReason {
    /// Prometheus `reason` label value. Adding a variant forces an
    /// update here (and in [`SyncSessionMetrics`]) — the matches are
    /// exhaustive, so it's compiler-enforced, not a documented invariant.
    fn label(self) -> &'static str {
        match self {
            DropReason::MailboxFull => "mailbox_full",
            DropReason::ActorClosed => "actor_closed",
            DropReason::ContextBusy => "context_busy",
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct DropReasonLabel {
    reason: String,
}

/// Prometheus + summary-log accounting for the sync-session actor
/// (#2319 Step 4).
///
/// Holds one owned [`Counter`] handle per [`DropReason`] — clones of the
/// entries in the `sync_session_jobs_dropped_total{reason=...}`
/// Prometheus family. The family itself stays in the registry;
/// `prometheus_client::Counter` is `Arc`-backed, so incrementing a
/// handle here updates the registered metric. `record_drop` dispatches
/// with an exhaustive `match` (no array indexing, no implicit
/// discriminant invariant — adding a [`DropReason`] won't compile until
/// the new field + match arms are added), is a single atomic increment
/// with no allocation, and `dropped_total` (the once-a-minute
/// `log_summary` line) is just the sum of the three — no second source
/// of truth. Cloned (cheaply) into the [`SyncSessionSender`] (mailbox
/// `Full`/`Closed` drops) and the [`SyncSessionActor`]
/// (per-context-busy drops).
#[derive(Clone, Debug)]
pub struct SyncSessionMetrics {
    mailbox_full: Counter,
    actor_closed: Counter,
    context_busy: Counter,
}

impl SyncSessionMetrics {
    /// Create and register metrics under the `sync_session` sub-registry.
    pub fn new(registry: &mut Registry) -> Self {
        let dropped = Family::<DropReasonLabel, Counter>::default();
        let sub = registry.sub_registry_with_prefix("sync_session");
        sub.register(
            "jobs_dropped_total",
            "SyncSessionJobs dropped without running, by reason",
            dropped.clone(),
        );
        Self::from_family(&dropped)
    }

    /// Unregistered variant for unit tests.
    #[cfg(test)]
    pub fn new_unregistered() -> Self {
        Self::from_family(&Family::<DropReasonLabel, Counter>::default())
    }

    fn from_family(dropped: &Family<DropReasonLabel, Counter>) -> Self {
        let counter = |reason: DropReason| {
            dropped
                .get_or_create(&DropReasonLabel {
                    reason: reason.label().to_owned(),
                })
                .clone()
        };
        Self {
            mailbox_full: counter(DropReason::MailboxFull),
            actor_closed: counter(DropReason::ActorClosed),
            context_busy: counter(DropReason::ContextBusy),
        }
    }

    fn counter(&self, reason: DropReason) -> &Counter {
        match reason {
            DropReason::MailboxFull => &self.mailbox_full,
            DropReason::ActorClosed => &self.actor_closed,
            DropReason::ContextBusy => &self.context_busy,
        }
    }

    /// Record one dropped job. Single atomic increment on the
    /// pre-created per-reason counter — no allocation.
    pub fn record_drop(&self, reason: DropReason) {
        let _prev = self.counter(reason).inc();
    }

    /// Total dropped jobs across all reasons (for `log_summary`).
    /// Derived from the per-reason counters — no duplicate atomic.
    pub fn dropped_total(&self) -> u64 {
        self.mailbox_full.get() + self.actor_closed.get() + self.context_busy.get()
    }

    /// Read the per-reason counter — used by `log_summary` (and tests
    /// verifying `record_drop` routes each reason to the right metric).
    fn count(&self, reason: DropReason) -> u64 {
        self.counter(reason).get()
    }
}

/// Result reported back to `SyncManager::start` for state tracking
/// (success-count, backoff). Mirrors the tuple the legacy
/// `FuturesUnordered`-based loop produced before #2316.
#[derive(Debug)]
pub struct SyncSessionResult {
    pub context_id: ContextId,
    pub peer_id: PeerId,
    pub took: Duration,
    /// `Ok(Ok(_))` = sync ran to completion; `Ok(Err(_))` = sync
    /// returned an error; `Err(_)` = session timed out.
    pub result: Result<Result<SyncProtocol, eyre::Error>, tokio::time::error::Elapsed>,
}

/// RAII guard that decrements [`SyncSessionActor::in_flight`] on
/// drop, including panic unwinds. Same pattern as
/// `state_delta_bridge::InFlightGuard`.
struct InFlightGuard {
    counter: Arc<AtomicU64>,
}

impl InFlightGuard {
    fn new(counter: Arc<AtomicU64>) -> Self {
        let _prev = counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let _prev = self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// RAII guard that removes a `ContextId` from
/// [`SyncSessionActor::in_flight_initiators`] on drop, including
/// panic/timeout/cancel unwinds (#2319).
struct ContextGuard {
    map: Arc<DashMap<ContextId, ()>>,
    context_id: ContextId,
}

impl Drop for ContextGuard {
    fn drop(&mut self) {
        let _removed = self.map.remove(&self.context_id);
    }
}

/// One unit of work routed to [`SyncSessionActor`].
#[derive(Message)]
#[rtype(result = "()")]
pub enum SyncSessionJob {
    /// Inbound sync stream from a peer; runs `handle_opened_stream`
    /// (which dispatches to the appropriate responder).
    Responder {
        peer_id: PeerId,
        stream: Box<Stream>,
    },
    /// Locally-driven sync attempt; runs `perform_interval_sync`.
    /// `peer_id = None` lets the manager choose a peer.
    Initiator {
        context_id: ContextId,
        peer_id: Option<PeerId>,
    },
}

/// Sender side. Wraps `Addr<SyncSessionActor>` so dispatch sites can
/// `try_send` without depending on Actix types directly.
#[derive(Clone, Debug)]
pub struct SyncSessionSender {
    addr: Addr<SyncSessionActor>,
    metrics: SyncSessionMetrics,
}

/// Error returned by [`SyncSessionSender::try_send`].
#[derive(Debug)]
pub enum SyncSessionSendError {
    /// Mailbox at capacity; drop and rely on periodic-sync retry.
    Full,
    /// Actor stopped — bridge is shutting down or has crashed.
    Closed,
}

impl SyncSessionSender {
    /// Non-blocking enqueue. Counts the drop (per-reason + total) on
    /// both `Full` and `Closed` so the periodic summary log and
    /// Prometheus don't undercount drops if the actor crashes or shuts
    /// down while the system is still running.
    pub fn try_send(&self, job: SyncSessionJob) -> Result<(), SyncSessionSendError> {
        match self.addr.try_send(job) {
            Ok(()) => Ok(()),
            Err(actix::dev::SendError::Full(_)) => {
                self.metrics.record_drop(DropReason::MailboxFull);
                Err(SyncSessionSendError::Full)
            }
            Err(actix::dev::SendError::Closed(_)) => {
                self.metrics.record_drop(DropReason::ActorClosed);
                Err(SyncSessionSendError::Closed)
            }
        }
    }
}

/// Sync-session dispatch actor. Runs on a dedicated Arbiter so a
/// long session (slow WASM merge-apply, divergent DAG) can't starve
/// the network/gossipsub task or the NodeManager mailbox.
pub struct SyncSessionActor {
    sync_manager: SyncManager,
    /// Outer per-session `tokio::time::timeout` — `sync_config.session_deadline`
    /// (#2319; defaults to the 30 s `sync_config.timeout`, the
    /// per-step budget, but is separately tunable). Bounds how long one
    /// stuck session can hold a concurrency slot and burn the arbiter
    /// thread.
    session_timeout: Duration,
    /// Caps concurrently-running sessions at `sync_config.max_concurrent`
    /// (default 30). The mailbox bounds *queued* jobs; this bounds
    /// *in-flight* jobs, restoring the limit the legacy
    /// `if futs.len() >= max_concurrent { advance().await }` check
    /// enforced before #2316. The acquire is unbounded — `acquire_owned`
    /// has no timeout — so the per-session `tokio::time::timeout` only
    /// applies to the work *after* the permit is held.
    concurrency: Arc<Semaphore>,
    /// One in-flight `Initiator` session per `ContextId` (#2319). A
    /// second initiator for the same context is dropped (counted as
    /// `ContextBusy`) rather than queued — two concurrent syncs of one
    /// context's state would race, and the dispatch loop's Phase-1
    /// "Sync already in progress" check was the only thing that
    /// prevented it before, which #2319 showed is not airtight under
    /// mailbox backpressure. Responder jobs are per-stream, not
    /// per-context, so they are not gated here; the global `concurrency`
    /// semaphore still caps total in-flight work.
    in_flight_initiators: Arc<DashMap<ContextId, ()>>,
    /// Initiator results are forwarded here so `SyncManager::start`
    /// can update its per-context tracking state. `None` means
    /// results are dropped (e.g. in unit tests). Unbounded because a
    /// dropped result would leave the per-context `last_sync = None`
    /// forever (no `on_success`/`on_failure` would run), permanently
    /// stalling that context — same failure shape as the C1 dispatch
    /// stall fixed earlier in #2317.
    result_tx: Option<mpsc::UnboundedSender<SyncSessionResult>>,
    metrics: SyncSessionMetrics,
    in_flight: Arc<AtomicU64>,
    processed_total: Arc<AtomicU64>,
    error_total: Arc<AtomicU64>,
    timeout_total: Arc<AtomicU64>,
}

impl SyncSessionActor {
    fn new(
        sync_manager: SyncManager,
        session_timeout: Duration,
        max_concurrent: usize,
        result_tx: Option<mpsc::UnboundedSender<SyncSessionResult>>,
        metrics: SyncSessionMetrics,
    ) -> Self {
        Self {
            sync_manager,
            session_timeout,
            concurrency: Arc::new(Semaphore::new(max_concurrent)),
            in_flight_initiators: Arc::new(DashMap::new()),
            result_tx,
            metrics,
            in_flight: Arc::new(AtomicU64::new(0)),
            processed_total: Arc::new(AtomicU64::new(0)),
            error_total: Arc::new(AtomicU64::new(0)),
            timeout_total: Arc::new(AtomicU64::new(0)),
        }
    }

    fn log_summary(&self) {
        let processed = self.processed_total.load(Ordering::Relaxed);
        let errors = self.error_total.load(Ordering::Relaxed);
        let timeouts = self.timeout_total.load(Ordering::Relaxed);
        let dropped = self.metrics.dropped_total();
        // #2319: `record_drop(ContextBusy)` already counts this; reuse
        // that Prometheus counter rather than a parallel atomic that
        // could drift.
        let per_context_busy = self.metrics.count(DropReason::ContextBusy);
        let in_flight = self.in_flight.load(Ordering::Relaxed);
        let in_flight_contexts = self.in_flight_initiators.len();
        info!(
            processed_total = processed,
            error_total = errors,
            timeout_total = timeouts,
            dropped_total = dropped,
            per_context_busy_total = per_context_busy,
            in_flight,
            in_flight_contexts,
            "SyncSession actor summary"
        );
    }
}

impl Actor for SyncSessionActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("SyncSession actor started on dedicated Arbiter");
        let _handle = ctx.run_interval(SUMMARY_INTERVAL, |actor, _ctx| {
            actor.log_summary();
        });
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        self.log_summary();
        info!("SyncSession actor stopped");
    }
}

impl Handler<SyncSessionJob> for SyncSessionActor {
    type Result = ();

    fn handle(&mut self, job: SyncSessionJob, ctx: &mut Self::Context) {
        let session_timeout = self.session_timeout;
        let sync_manager = self.sync_manager.clone();
        let result_tx = self.result_tx.clone();
        let concurrency = Arc::clone(&self.concurrency);

        match job {
            SyncSessionJob::Responder { peer_id, stream } => {
                let in_flight_guard = InFlightGuard::new(Arc::clone(&self.in_flight));
                // Responder: `handle_opened_stream` returns `()` so
                // there is no `error_total` distinction here — only
                // `processed_total` and `timeout_total`.
                let processed_total = Arc::clone(&self.processed_total);
                let timeout_total = Arc::clone(&self.timeout_total);
                let work = async move {
                    let _guard = in_flight_guard;
                    let started = Instant::now();
                    // #2319: one `timeout` covers BOTH waiting for a
                    // concurrency permit and running the session, so
                    // total wall time per responder slot is bounded by
                    // `session_timeout`. Without bounding the acquire, a
                    // saturated actor (every `max_concurrent` slot held
                    // by a stuck session) would park this job
                    // indefinitely and pin the peer's inbound stream
                    // open until the peer itself gives up.
                    // Pin the session so a timeout can re-await it (bounded)
                    // rather than dropping it. Dropping a responder future
                    // mid-step can leave local storage and the DAG partially
                    // applied and diverged from the peer; a clean completion
                    // keeps them consistent.
                    let mut session = pin!(async move {
                        let _permit = concurrency.acquire_owned().await.ok();
                        sync_manager.handle_opened_stream(peer_id, stream).await
                    });
                    let outcome = match tokio::time::timeout(session_timeout, &mut session).await {
                        Ok(()) => Ok(()),
                        Err(_elapsed) => {
                            // Bounded grace re-await: give a slow-but-progressing
                            // session one more window to finish CLEANLY before
                            // giving up. The grace is deliberately shorter than
                            // `session_timeout` so total wall time stays under
                            // the 2× watchdog grace (a permit-starved session
                            // must not trip a spurious synthetic failure).
                            //
                            // TRADEOFF (deliberate): the permit lives INSIDE
                            // `session`, so if the session already acquired one it
                            // keeps holding it for the grace window — a stuck
                            // session pins its slot for up to 1.5× `session_timeout`
                            // rather than releasing at 1×. This is the accepted
                            // cost of not dropping a possibly-mid-protocol future:
                            // it is still strictly bounded (unlike an unbounded
                            // re-await) and stays under the 2× watchdog grace, so
                            // it cannot deadlock the actor the way an unbounded
                            // hold would. A session that timed out while STILL
                            // waiting for a permit holds nothing, so grace-extending
                            // it is free.
                            //
                            // We do NOT warn yet — a session that finishes within
                            // the grace completed successfully, so warning here
                            // would log a spurious timeout for a healthy session.
                            match tokio::time::timeout(session_timeout / 2, &mut session).await {
                                Ok(()) => {
                                    debug!(
                                        %peer_id,
                                        timeout_secs = session_timeout.as_secs(),
                                        "SyncSession responder exceeded soft timeout but finished within the grace window"
                                    );
                                    Ok(())
                                }
                                // Grace ALSO elapsed → genuinely stuck. Drop it so
                                // its concurrency permit is released and other
                                // sessions aren't starved (#2319 rationale); the
                                // downstream arm logs the drop. The next periodic
                                // sync repairs the rare divergence a drop leaves.
                                Err(elapsed) => Err(elapsed),
                            }
                        }
                    };
                    match &outcome {
                        Ok(()) => {
                            let _prev = processed_total.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(_elapsed) => {
                            let _prev = timeout_total.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    (outcome, started)
                };

                let _spawn_handle = ctx.spawn(work.into_actor(self).map(
                    move |(outcome, started), _act, _ctx| match outcome {
                        Ok(()) => {
                            debug!(
                                %peer_id,
                                elapsed_ms = started.elapsed().as_millis(),
                                "SyncSession responder completed"
                            );
                        }
                        Err(_elapsed) => {
                            warn!(
                                %peer_id,
                                timeout_secs = session_timeout.as_secs(),
                                elapsed_ms = started.elapsed().as_millis(),
                                "SyncSession responder exceeded timeout — dropping; peer will retry"
                            );
                        }
                    },
                ));
            }
            SyncSessionJob::Initiator {
                context_id,
                peer_id,
            } => {
                // #2319: refuse a duplicate initiator for this context.
                if self.in_flight_initiators.insert(context_id, ()).is_some() {
                    self.metrics.record_drop(DropReason::ContextBusy);
                    debug!(
                        %context_id,
                        "SyncSession actor already running an initiator for this context — dropping duplicate (#2319)"
                    );
                    // We accepted this job off the mailbox (`try_send`
                    // returned `Ok`), so `SyncManager::start` has already
                    // run Phase 3 and cleared the context's `last_sync`
                    // to `None` ("in progress"). If we returned without
                    // a result the context would stay `None` forever —
                    // the exact #2317 permanent-stall. Send a synthetic
                    // failure so `apply_session_result` clears it and the
                    // periodic loop retries on the next eligible tick.
                    if let Some(tx) = &result_tx {
                        let _ignored = tx.send(SyncSessionResult {
                            context_id,
                            peer_id: peer_id.unwrap_or_else(PeerId::random),
                            took: Duration::ZERO,
                            result: Ok(Err(eyre::eyre!(
                                "initiator skipped — a sync session for this context is already in flight on the actor (#2319)"
                            ))),
                        });
                    }
                    return;
                }
                let context_guard = ContextGuard {
                    map: Arc::clone(&self.in_flight_initiators),
                    context_id,
                };
                let in_flight_guard = InFlightGuard::new(Arc::clone(&self.in_flight));
                let processed_total = Arc::clone(&self.processed_total);
                let error_total = Arc::clone(&self.error_total);
                let timeout_total = Arc::clone(&self.timeout_total);
                let work = async move {
                    let _context_guard = context_guard;
                    let _guard = in_flight_guard;
                    let started = Instant::now();
                    // #2319: one `timeout` covers BOTH waiting for a
                    // concurrency permit and running `perform_interval_sync`,
                    // so total wall time is bounded by `session_timeout`
                    // (not 2×, which two separate timeouts gave — and 2×
                    // is the watchdog's whole grace, so a permit-starved
                    // session could otherwise trip a spurious synthetic
                    // failure while still legitimately running). If every
                    // slot is held by sessions stuck in a synchronous
                    // merge loop the `timeout` can't preempt, an
                    // unbounded `acquire_owned().await` would park this
                    // initiator forever — and since it accepted the job
                    // off the mailbox, `SyncManager::start` already
                    // cleared this context's `last_sync` to `None`, so
                    // with no result the context stays "in progress"
                    // permanently. On timeout the `Err(Elapsed)` outcome
                    // below still yields a (failure) result, so
                    // `apply_session_result` clears the flag and the
                    // periodic loop retries.
                    // Pin the session so a timeout can re-await it (bounded)
                    // instead of dropping the in-flight interval-sync future
                    // mid-step (which can leave storage/DAG diverged).
                    let mut session = pin!(async move {
                        let _permit = concurrency.acquire_owned().await.ok();
                        sync_manager
                            .perform_interval_sync(context_id, peer_id)
                            .await
                    });
                    let outcome = match tokio::time::timeout(session_timeout, &mut session).await {
                        Ok(res) => Ok(res),
                        Err(_elapsed) => {
                            // Bounded grace re-await (see the responder path for
                            // the rationale): let a slow-but-progressing session
                            // finish cleanly, but cap the grace below
                            // `session_timeout` so total wall time stays under
                            // the 2× watchdog grace. No warn here — a session that
                            // finishes within the grace succeeded, so warning
                            // would log a spurious timeout for a healthy session.
                            match tokio::time::timeout(session_timeout / 2, &mut session).await {
                                Ok(res) => {
                                    debug!(
                                        %context_id,
                                        timeout_secs = session_timeout.as_secs(),
                                        "SyncSession initiator exceeded soft timeout but finished within the grace window"
                                    );
                                    Ok(res)
                                }
                                // Grace also elapsed → drop it so the permit is
                                // released (#2319); the downstream arm logs it.
                                Err(elapsed) => Err(elapsed),
                            }
                        }
                    };

                    let chosen_peer = outcome
                        .as_ref()
                        .ok()
                        .and_then(|r| r.as_ref().ok())
                        .map(|(p, _)| *p)
                        .or(peer_id)
                        .unwrap_or_else(PeerId::random);

                    match &outcome {
                        Ok(Ok(_)) => {
                            let _prev = processed_total.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(Err(_)) => {
                            let _prev = error_total.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(_elapsed) => {
                            let _prev = timeout_total.fetch_add(1, Ordering::Relaxed);
                        }
                    }

                    let took = started.elapsed();
                    let result = outcome.map(|r| r.map(|(_, proto)| proto));
                    (result, took, chosen_peer)
                };

                let _spawn_handle = ctx.spawn(work.into_actor(self).map(
                    move |(result, took, chosen_peer), _act, _ctx| {
                        match &result {
                            Ok(Ok(_)) => debug!(
                                %context_id,
                                %chosen_peer,
                                took_ms = took.as_millis(),
                                "SyncSession initiator completed"
                            ),
                            Ok(Err(err)) => debug!(
                                %context_id,
                                %chosen_peer,
                                took_ms = took.as_millis(),
                                error = %err,
                                "SyncSession initiator failed"
                            ),
                            Err(_elapsed) => warn!(
                                %context_id,
                                %chosen_peer,
                                took_ms = took.as_millis(),
                                "SyncSession initiator exceeded timeout — dropping; periodic-sync will retry"
                            ),
                        }

                        if let Some(tx) = result_tx {
                            let session_result = SyncSessionResult {
                                context_id,
                                peer_id: chosen_peer,
                                took,
                                result,
                            };
                            // Unbounded: the only error is "receiver
                            // gone" (SyncManager loop shut down), and
                            // we don't need to retry in that case.
                            let _ignored = tx.send(session_result);
                        }
                    },
                ));
            }
        }
    }
}

/// Boot the [`SyncSessionActor`] on the supplied dedicated Arbiter
/// and return a [`SyncSessionSender`] for dispatch sites to hold.
///
/// `capacity` bounds the mailbox (queued jobs); `max_concurrent`
/// bounds the in-flight semaphore (running sessions) — together they
/// recreate the legacy `FuturesUnordered` queue + `max_concurrent`
/// cap that `SyncManager::start` enforced before #2316.
///
/// `session_deadline` is the outer per-session `tokio::time::timeout`
/// (#2319 — pass `config.sync.session_deadline`; defaults to the 30 s
/// `config.sync.timeout` so cold-start snapshot syncs aren't cut off).
/// Drop counters are registered under `sync_session` in `registry`.
///
/// `result_tx` is the channel `SyncManager::start` reads from to
/// update its per-context tracking state. Pass `None` to discard
/// results (used in unit tests).
pub fn start_sync_session_actor(
    arbiter: &ArbiterHandle,
    capacity: usize,
    max_concurrent: usize,
    sync_manager: SyncManager,
    session_deadline: Duration,
    result_tx: Option<mpsc::UnboundedSender<SyncSessionResult>>,
    registry: &mut Registry,
) -> SyncSessionSender {
    let metrics = SyncSessionMetrics::new(registry);
    let metrics_for_actor = metrics.clone();

    let addr = SyncSessionActor::start_in_arbiter(arbiter, move |ctx| {
        ctx.set_mailbox_capacity(capacity);
        SyncSessionActor::new(
            sync_manager,
            session_deadline,
            max_concurrent,
            result_tx,
            metrics_for_actor,
        )
    });

    SyncSessionSender { addr, metrics }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_start_at_zero() {
        let m = SyncSessionMetrics::new_unregistered();
        assert_eq!(m.dropped_total(), 0);
        assert_eq!(m.count(DropReason::MailboxFull), 0);
        assert_eq!(m.count(DropReason::ActorClosed), 0);
        assert_eq!(m.count(DropReason::ContextBusy), 0);
    }

    #[test]
    fn record_drop_routes_each_reason_to_its_own_counter() {
        let m = SyncSessionMetrics::new_unregistered();
        m.record_drop(DropReason::MailboxFull);
        m.record_drop(DropReason::MailboxFull);
        m.record_drop(DropReason::ActorClosed);
        m.record_drop(DropReason::ContextBusy);
        assert_eq!(m.count(DropReason::MailboxFull), 2);
        assert_eq!(m.count(DropReason::ActorClosed), 1);
        assert_eq!(m.count(DropReason::ContextBusy), 1);
        assert_eq!(m.dropped_total(), 4);
    }

    #[test]
    fn dropped_total_is_the_sum_of_per_reason_counters() {
        let m = SyncSessionMetrics::new_unregistered();
        for _ in 0..3 {
            m.record_drop(DropReason::ContextBusy);
        }
        m.record_drop(DropReason::MailboxFull);
        assert_eq!(
            m.dropped_total(),
            m.count(DropReason::MailboxFull)
                + m.count(DropReason::ActorClosed)
                + m.count(DropReason::ContextBusy)
        );
        assert_eq!(m.dropped_total(), 4);
    }
}
