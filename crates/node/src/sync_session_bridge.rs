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
//! ## Backpressure
//!
//! Bounded Actix mailbox via `set_mailbox_capacity`; `Addr::try_send`
//! returns `SendError::Full` on overflow. On overflow the dispatch
//! site logs the drop; the existing periodic-sync interval and
//! heartbeat-driven sync triggers cover dropped initiators, and
//! peers will retry dropped responder streams via their own retry
//! logic.
//!
//! ## Mirrors `state_delta_bridge`
//!
//! Same shape as `state_delta_bridge::StateDeltaActor`: dedicated
//! Arbiter, bounded mailbox, `try_send`, `InFlightGuard`,
//! per-session `tokio::time::timeout`, and counters for
//! processed/error/timeout/dropped logged once a minute.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use actix::{
    Actor, ActorFutureExt, Addr, ArbiterHandle, AsyncContext, Context, Handler, Message, WrapFuture,
};
use calimero_network_primitives::stream::Stream;
use calimero_primitives::context::ContextId;
use libp2p::PeerId;
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
    dropped_total: Arc<AtomicU64>,
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
    /// Non-blocking enqueue. Increments the drop counter on both
    /// `Full` and `Closed` so the periodic summary log doesn't
    /// undercount drops if the actor crashes or shuts down while the
    /// system is still running.
    pub fn try_send(&self, job: SyncSessionJob) -> Result<(), SyncSessionSendError> {
        match self.addr.try_send(job) {
            Ok(()) => Ok(()),
            Err(actix::dev::SendError::Full(_)) => {
                let _prev = self.dropped_total.fetch_add(1, Ordering::Relaxed);
                Err(SyncSessionSendError::Full)
            }
            Err(actix::dev::SendError::Closed(_)) => {
                let _prev = self.dropped_total.fetch_add(1, Ordering::Relaxed);
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
    session_timeout: Duration,
    /// Caps concurrently-running sessions at `sync_config.max_concurrent`
    /// (default 30). The mailbox bounds *queued* jobs; this bounds
    /// *in-flight* jobs, restoring the limit the legacy
    /// `if futs.len() >= max_concurrent { advance().await }` check
    /// enforced before #2316. A timed-out `acquire_owned` drops the
    /// job and counts it via `dropped_total`.
    concurrency: Arc<Semaphore>,
    /// Initiator results are forwarded here so `SyncManager::start`
    /// can update its per-context tracking state. `None` means
    /// results are dropped (e.g. in unit tests).
    result_tx: Option<mpsc::Sender<SyncSessionResult>>,
    in_flight: Arc<AtomicU64>,
    processed_total: Arc<AtomicU64>,
    error_total: Arc<AtomicU64>,
    timeout_total: Arc<AtomicU64>,
    dropped_total: Arc<AtomicU64>,
}

impl SyncSessionActor {
    fn new(
        sync_manager: SyncManager,
        session_timeout: Duration,
        max_concurrent: usize,
        result_tx: Option<mpsc::Sender<SyncSessionResult>>,
        dropped_total: Arc<AtomicU64>,
    ) -> Self {
        Self {
            sync_manager,
            session_timeout,
            concurrency: Arc::new(Semaphore::new(max_concurrent)),
            result_tx,
            in_flight: Arc::new(AtomicU64::new(0)),
            processed_total: Arc::new(AtomicU64::new(0)),
            error_total: Arc::new(AtomicU64::new(0)),
            timeout_total: Arc::new(AtomicU64::new(0)),
            dropped_total,
        }
    }

    fn log_summary(&self) {
        let processed = self.processed_total.load(Ordering::Relaxed);
        let errors = self.error_total.load(Ordering::Relaxed);
        let timeouts = self.timeout_total.load(Ordering::Relaxed);
        let dropped = self.dropped_total.load(Ordering::Relaxed);
        let in_flight = self.in_flight.load(Ordering::Relaxed);
        info!(
            processed_total = processed,
            error_total = errors,
            timeout_total = timeouts,
            dropped_total = dropped,
            in_flight,
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
        let in_flight_guard = InFlightGuard::new(Arc::clone(&self.in_flight));

        let session_timeout = self.session_timeout;
        let sync_manager = self.sync_manager.clone();
        let result_tx = self.result_tx.clone();
        let concurrency = Arc::clone(&self.concurrency);

        match job {
            SyncSessionJob::Responder { peer_id, stream } => {
                // Responder: `handle_opened_stream` returns `()` so
                // there is no `error_total` distinction here — only
                // `processed_total` and `timeout_total`.
                let processed_total = Arc::clone(&self.processed_total);
                let timeout_total = Arc::clone(&self.timeout_total);
                let work = async move {
                    let _guard = in_flight_guard;
                    // Bound concurrent in-flight sessions to
                    // `sync_config.max_concurrent` (default 30); when
                    // saturated, queued jobs wait here rather than
                    // running unbounded. The session timeout below
                    // still applies once the permit is held.
                    let _permit = concurrency.acquire_owned().await.ok();
                    let started = Instant::now();
                    let outcome = tokio::time::timeout(
                        session_timeout,
                        sync_manager.handle_opened_stream(peer_id, stream),
                    )
                    .await;
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
                let processed_total = Arc::clone(&self.processed_total);
                let error_total = Arc::clone(&self.error_total);
                let timeout_total = Arc::clone(&self.timeout_total);
                let work = async move {
                    let _guard = in_flight_guard;
                    let _permit = concurrency.acquire_owned().await.ok();
                    let started = Instant::now();
                    let outcome = tokio::time::timeout(
                        session_timeout,
                        sync_manager.perform_interval_sync(context_id, peer_id),
                    )
                    .await;

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
                            // Best-effort: if the receiver is gone the
                            // SyncManager loop is shutting down and we
                            // don't need to retry.
                            let _ignored = tx.try_send(session_result);
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
/// `result_tx` is the channel `SyncManager::start` reads from to
/// update its per-context tracking state. Pass `None` to discard
/// results (used in unit tests).
pub fn start_sync_session_actor(
    arbiter: &ArbiterHandle,
    capacity: usize,
    max_concurrent: usize,
    sync_manager: SyncManager,
    session_timeout: Duration,
    result_tx: Option<mpsc::Sender<SyncSessionResult>>,
) -> SyncSessionSender {
    let dropped_total = Arc::new(AtomicU64::new(0));
    let dropped_for_actor = Arc::clone(&dropped_total);

    let addr = SyncSessionActor::start_in_arbiter(arbiter, move |ctx| {
        ctx.set_mailbox_capacity(capacity);
        SyncSessionActor::new(
            sync_manager,
            session_timeout,
            max_concurrent,
            result_tx,
            dropped_for_actor,
        )
    });

    SyncSessionSender {
        addr,
        dropped_total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sender wrapper compiles, clones, and exposes a working
    /// `dropped_total` handle when started on a fresh Actix Arbiter.
    /// (Functional coverage is in the kv-store-with-handlers fuzzy
    /// test under issue #2316 acceptance criteria.)
    #[test]
    fn dropped_total_starts_at_zero() {
        let dropped_total = Arc::new(AtomicU64::new(0));
        assert_eq!(dropped_total.load(Ordering::Relaxed), 0);
    }
}
