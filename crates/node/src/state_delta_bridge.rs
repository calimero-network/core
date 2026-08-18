//! State delta processing actor.
//!
//! Moves `BroadcastMessage::StateDelta` processing off `NodeManager`'s
//! single Arbiter onto a dedicated `StateDeltaActor` running on its own
//! Arbiter (issue #2299, Layer 1).
//!
//! Why an Actix actor (not a tokio task): `handle_state_delta` holds a
//! non-`Send` `Box<dyn Iterator>` across an `await` inside the
//! `delta_store` (the persisted-deltas scan). Tokio's multi-threaded
//! `spawn` rejects non-`Send` futures. Actix's `ctx.spawn` runs on
//! the actor's local context, which doesn't require `Send` — same
//! semantics the original `ctx.spawn(...)` site in
//! `network_event.rs` was already using, just on a dedicated Arbiter
//! that no other variant shares.
//!
//! Backpressure: bounded Actix mailbox via `set_mailbox_capacity`;
//! `Addr::try_send` returns `SendError::Full` on overflow. The
//! dispatch site logs the drop; existing heartbeat-driven rebroadcast
//! covers it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use actix::{
    Actor, ActorFutureExt, Addr, ArbiterHandle, AsyncContext, Context, Handler, Message, WrapFuture,
};
use tracing::{debug, info, warn};

use crate::handlers::state_delta::{handle_state_delta, StateDeltaContext, StateDeltaMessage};

/// Mailbox capacity. At observed peak rate of ~10 StateDelta/sec
/// (issue #2299), 2048 covers a ~3-minute burst before dropping. On
/// overflow we drop and rely on the existing heartbeat-driven
/// rebroadcast path.
pub const STATE_DELTA_CHANNEL_CAPACITY: usize = 2048;

/// Soft per-delta processing budget. *Not* a hard cap: `handle_state_delta`
/// drives `context_client.execute`, whose WASM merge-apply runs on a
/// `spawn_blocking` thread that can't be cancelled. Abandoning the job on a
/// hard timeout would release the DAG write lock while that apply completes
/// and `commit()`s its writes anyway — late, racing the next delta and
/// leaving storage holding a delta the DAG doesn't have. So exceeding this
/// threshold only logs a warning and bumps `over_budget_total`; the job runs
/// to completion. The durable fix for the underlying slowness is #2199/#2238.
const STATE_DELTA_PROCESSING_TIMEOUT: Duration = Duration::from_secs(60);

/// Hard ceiling on concurrently in-flight jobs.
///
/// The mailbox bound alone does not cap concurrency: `handle` hands each job
/// to `ctx.spawn` and returns, so the mailbox slot frees immediately and the
/// job's future accumulates alongside every other still-running one. Anything
/// that parks a job for seconds — a peer that gossips deltas for a context
/// whose application bytecode it never serves, a slow merge-apply — therefore
/// grows in-flight futures at the full delta arrival rate with nothing to stop
/// it, each holding its delta payload and store handles.
///
/// Healthy processing completes in milliseconds, so steady-state in-flight
/// sits in the single digits and this never trips. It is deliberately well
/// under [`STATE_DELTA_CHANNEL_CAPACITY`]: the mailbox is sized to absorb a
/// multi-minute *arrival* burst, which is a different quantity from how many
/// jobs may run at once.
const STATE_DELTA_MAX_IN_FLIGHT: u64 = 256;

/// Periodic summary log interval.
const SUMMARY_INTERVAL: Duration = Duration::from_secs(60);

/// RAII guard that decrements [`StateDeltaActor::in_flight`] on
/// drop, including panic unwinds. Without this, a panic inside
/// `handle_state_delta` would skip the post-`.map(...)` decrement
/// path and leave a phantom in-flight count in the summary log.
struct InFlightGuard {
    counter: Arc<AtomicU64>,
}

impl InFlightGuard {
    /// Reserve one in-flight slot, or return `None` when `ceiling` is already
    /// reached.
    ///
    /// The check and the increment are one `fetch_update` rather than a load
    /// followed by a `fetch_add`: decrements come from guard drops in spawned
    /// futures, so a split check-then-increment could admit a job on a count
    /// that went stale between the two operations.
    fn try_acquire(counter: Arc<AtomicU64>, ceiling: u64) -> Option<Self> {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < ceiling).then(|| current + 1)
            })
            .ok()
            .map(|_prev| Self { counter })
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let _prev = self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// One unit of work routed to the [`StateDeltaActor`]. The dispatch
/// site in `network_event.rs` builds this from the deserialized
/// `BroadcastMessage::StateDelta` variant.
#[derive(Message)]
#[rtype(result = "()")]
pub struct StateDeltaJob {
    pub(crate) context: StateDeltaContext,
    pub(crate) message: StateDeltaMessage,
}

/// Sender side. Wraps `Addr<StateDeltaActor>` so the dispatch site
/// can `try_send` without depending on Actix types directly.
#[derive(Clone, Debug)]
pub struct StateDeltaSender {
    addr: Addr<StateDeltaActor>,
    dropped_total: Arc<AtomicU64>,
}

/// Error returned by [`StateDeltaSender::try_send`] when the actor's
/// mailbox is full or the actor has stopped.
#[derive(Debug)]
pub enum StateDeltaSendError {
    /// Mailbox at capacity; drop and rely on heartbeat rebroadcast.
    Full,
    /// Actor stopped — bridge is shutting down or has crashed.
    Closed,
}

impl StateDeltaSender {
    /// Non-blocking enqueue. Increments the drop counter on
    /// `Full`. Errors are returned so the caller can log per-message
    /// context (context_id, delta_id) at the dispatch site.
    pub fn try_send(&self, job: StateDeltaJob) -> Result<(), StateDeltaSendError> {
        match self.addr.try_send(job) {
            Ok(()) => Ok(()),
            Err(actix::dev::SendError::Full(_)) => {
                let _prev = self.dropped_total.fetch_add(1, Ordering::Relaxed);
                Err(StateDeltaSendError::Full)
            }
            Err(actix::dev::SendError::Closed(_)) => Err(StateDeltaSendError::Closed),
        }
    }
}

/// State delta processing actor. Runs on a dedicated Arbiter so its
/// `ctx.spawn`'d work doesn't compete with `NodeManager`'s sync /
/// heartbeat / blob / namespace handlers for the same thread.
pub struct StateDeltaActor {
    in_flight: Arc<AtomicU64>,
    /// Successful `handle_state_delta` returns.
    processed_total: Arc<AtomicU64>,
    /// Failed `handle_state_delta` returns (decryption, DAG apply,
    /// handler exec). Distinct from `over_budget_total`.
    error_total: Arc<AtomicU64>,
    /// Jobs that exceeded [`STATE_DELTA_PROCESSING_TIMEOUT`] (and kept
    /// running — see that const). Separate from `error_total` so a
    /// slow-merge storm is distinguishable from an application-error
    /// storm in the summary log.
    over_budget_total: Arc<AtomicU64>,
    /// Jobs refused because [`STATE_DELTA_MAX_IN_FLIGHT`] was already
    /// reached. Kept apart from `dropped_total` (mailbox overflow) so a
    /// concurrency stall is distinguishable from an arrival-rate burst.
    shed_total: Arc<AtomicU64>,
    dropped_total: Arc<AtomicU64>,
}

impl StateDeltaActor {
    fn new(dropped_total: Arc<AtomicU64>) -> Self {
        Self {
            in_flight: Arc::new(AtomicU64::new(0)),
            processed_total: Arc::new(AtomicU64::new(0)),
            error_total: Arc::new(AtomicU64::new(0)),
            over_budget_total: Arc::new(AtomicU64::new(0)),
            shed_total: Arc::new(AtomicU64::new(0)),
            dropped_total,
        }
    }

    fn log_summary(&self) {
        let processed = self.processed_total.load(Ordering::Relaxed);
        let errors = self.error_total.load(Ordering::Relaxed);
        let over_budget = self.over_budget_total.load(Ordering::Relaxed);
        let shed = self.shed_total.load(Ordering::Relaxed);
        let dropped = self.dropped_total.load(Ordering::Relaxed);
        let in_flight = self.in_flight.load(Ordering::Relaxed);
        info!(
            processed_total = processed,
            error_total = errors,
            over_budget_total = over_budget,
            shed_total = shed,
            dropped_total = dropped,
            in_flight,
            max_in_flight = STATE_DELTA_MAX_IN_FLIGHT,
            "StateDelta actor summary"
        );
    }
}

impl Actor for StateDeltaActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("StateDelta actor started on dedicated Arbiter");
        let _handle = ctx.run_interval(SUMMARY_INTERVAL, |actor, _ctx| {
            actor.log_summary();
        });
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        self.log_summary();
        info!("StateDelta actor stopped");
    }
}

impl Handler<StateDeltaJob> for StateDeltaActor {
    type Result = ();

    fn handle(&mut self, job: StateDeltaJob, ctx: &mut Self::Context) {
        let processed_total = Arc::clone(&self.processed_total);
        let error_total = Arc::clone(&self.error_total);
        let over_budget_total = Arc::clone(&self.over_budget_total);

        let StateDeltaJob { context, message } = job;
        let context_id = message.context_id;
        let delta_id = message.delta_id;

        // RAII guard so `in_flight` is decremented even on panic. Acquiring it
        // is also the admission check: at the ceiling the job is shed here
        // rather than spawned, so a stalled workload cannot keep growing the
        // set of parked futures. Shedding costs this delta a redelivery, which
        // the heartbeat-driven rebroadcast already covers — the same trade the
        // mailbox-overflow path makes.
        let Some(in_flight_guard) =
            InFlightGuard::try_acquire(Arc::clone(&self.in_flight), STATE_DELTA_MAX_IN_FLIGHT)
        else {
            let _prev = self.shed_total.fetch_add(1, Ordering::Relaxed);
            warn!(
                %context_id,
                ?delta_id,
                max_in_flight = STATE_DELTA_MAX_IN_FLIGHT,
                "StateDelta job shed — in-flight ceiling reached; awaiting rebroadcast"
            );
            crate::node_metrics::record_delta_outcome("shed_in_flight_ceiling");
            return;
        };

        // Counters are incremented INSIDE `work`, before `_guard`
        // drops, so a summary log between guard-drop and the .map()
        // closure can never observe `in_flight=0` with stale totals.
        let work = async move {
            let _guard = in_flight_guard;
            let started = Instant::now();
            let fut = handle_state_delta(context, message);
            tokio::pin!(fut);
            // Soft budget: if `handle_state_delta` runs long, warn and bump
            // a counter, but DO NOT abandon it. Its downstream WASM apply
            // runs on a `spawn_blocking` thread that can't be cancelled;
            // dropping this future would release the DAG write lock while
            // that apply completes and commits late, racing the next delta
            // (storage holds a delta the DAG doesn't — re-synced, re-applied,
            // divergent). The merge-apply is gas-bounded so it terminates.
            // See #2199 / #2238.
            let result = match tokio::time::timeout(STATE_DELTA_PROCESSING_TIMEOUT, &mut fut).await
            {
                Ok(r) => r,
                Err(_elapsed) => {
                    let _prev = over_budget_total.fetch_add(1, Ordering::Relaxed);
                    warn!(
                        %context_id,
                        ?delta_id,
                        over_budget_secs = STATE_DELTA_PROCESSING_TIMEOUT.as_secs(),
                        "StateDelta worker over soft budget — still processing (a spawn_blocking apply can't be cancelled without leaving storage/DAG divergent); see #2199/#2238"
                    );
                    fut.await
                }
            };
            match &result {
                Ok(()) => {
                    let _prev = processed_total.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    let _prev = error_total.fetch_add(1, Ordering::Relaxed);
                }
            }
            (result, started)
        };

        let _spawn_handle = ctx.spawn(work.into_actor(self).map(
            move |(result, started), _act, _ctx| match result {
                Ok(()) => {
                    debug!(
                        %context_id,
                        ?delta_id,
                        elapsed_ms = started.elapsed().as_millis(),
                        "StateDelta worker completed"
                    );
                }
                Err(err) => {
                    warn!(?err, %context_id, ?delta_id, "Failed to handle state delta");
                }
            },
        ));
    }
}

/// Boot the [`StateDeltaActor`] on the supplied dedicated Arbiter
/// and return a [`StateDeltaSender`] for the dispatch site to hold.
///
/// The Actix `System` lives on a different thread from the tokio
/// runtime in this codebase (`ArbiterPool` runs `System::new()` in
/// `spawn_blocking`), so callers obtain an `ArbiterHandle` from the
/// pool and pass it here rather than letting this function call
/// `Arbiter::new()` itself — the latter only works when a `System`
/// is registered on the calling thread.
pub fn start_state_delta_actor(arbiter: &ArbiterHandle, capacity: usize) -> StateDeltaSender {
    let dropped_total = Arc::new(AtomicU64::new(0));
    let dropped_for_actor = Arc::clone(&dropped_total);

    // Set the mailbox capacity in the constructor closure (before any
    // message arrives) rather than in `started()`, so the bound is in
    // effect for every queued message — not just those received after
    // the actor's first lifecycle hook.
    let addr = StateDeltaActor::start_in_arbiter(arbiter, move |ctx| {
        ctx.set_mailbox_capacity(capacity);
        StateDeltaActor::new(dropped_for_actor)
    });

    StateDeltaSender {
        addr,
        dropped_total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sender wrapper compiles, clones, and exposes a working
    /// `dropped_total` handle when started on a fresh Actix Arbiter
    /// inside an Actix `System` (which `#[actix::test]` provides).
    #[actix::test]
    async fn sender_clones_and_starts_with_zero_drops() {
        let arbiter = actix::Arbiter::new();
        let sender = start_state_delta_actor(&arbiter.handle(), 8);
        assert_eq!(sender.dropped_total.load(Ordering::Relaxed), 0);
        let _clone = sender.clone();
        let _stopped = arbiter.stop();
    }

    /// Slots are handed out up to the ceiling and refused past it, so a
    /// workload that parks jobs cannot grow the in-flight set without bound.
    #[test]
    fn try_acquire_refuses_past_the_ceiling() {
        let counter = Arc::new(AtomicU64::new(0));

        let first = InFlightGuard::try_acquire(Arc::clone(&counter), 2);
        let second = InFlightGuard::try_acquire(Arc::clone(&counter), 2);
        assert!(first.is_some());
        assert!(second.is_some());
        assert_eq!(counter.load(Ordering::Relaxed), 2);

        let refused = InFlightGuard::try_acquire(Arc::clone(&counter), 2);
        assert!(refused.is_none());
        // A refusal must not consume a slot, or a shedding actor would ratchet
        // its own count upward and never admit another job.
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    /// Dropping a guard frees its slot for the next job.
    #[test]
    fn dropping_a_guard_readmits() {
        let counter = Arc::new(AtomicU64::new(0));

        let guard = InFlightGuard::try_acquire(Arc::clone(&counter), 1);
        assert!(guard.is_some());
        assert!(InFlightGuard::try_acquire(Arc::clone(&counter), 1).is_none());

        drop(guard);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        assert!(InFlightGuard::try_acquire(Arc::clone(&counter), 1).is_some());
    }

    /// A zero ceiling admits nothing — guards the boundary arithmetic in
    /// `try_acquire`'s `current < ceiling` test.
    #[test]
    fn zero_ceiling_admits_nothing() {
        let counter = Arc::new(AtomicU64::new(0));
        assert!(InFlightGuard::try_acquire(Arc::clone(&counter), 0).is_none());
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    // Functional tests of `handle_state_delta` itself live in the
    // existing `crates/node/src/handlers/state_delta/mod.rs::tests`
    // module and in the kv-store-with-handlers fuzzy load test (issue
    // #2299 acceptance criteria). The bridge's contract is "delivers
    // the job to a dedicated Arbiter with bounded mailbox" — Actix's
    // own test suite covers `set_mailbox_capacity` and `try_send`.
}
