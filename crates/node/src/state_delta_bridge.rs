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
    Actor, ActorFutureExt, Addr, ArbiterHandle, AsyncContext, Context, Handler, Message,
    WrapFuture,
};
use tracing::{debug, info, warn};

use crate::handlers::state_delta::{handle_state_delta, StateDeltaContext, StateDeltaMessage};

/// Mailbox capacity. At observed peak rate of ~10 StateDelta/sec
/// (issue #2299), 2048 covers a ~3-minute burst before dropping. On
/// overflow we drop and rely on the existing heartbeat-driven
/// rebroadcast path.
pub const STATE_DELTA_CHANNEL_CAPACITY: usize = 2048;

/// Reserved for a future Layer 2 per-context concurrency cap. Layer 1
/// relies on the actor's single-Arbiter cooperative scheduling — no
/// explicit semaphore. Kept here so callers can read a public
/// constant if they ever need to size a peer-side test.
pub const STATE_DELTA_PARALLELISM: usize = 32;

/// Periodic summary log interval.
const SUMMARY_INTERVAL: Duration = Duration::from_secs(60);

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
    capacity: usize,
    in_flight: Arc<AtomicU64>,
    processed_total: Arc<AtomicU64>,
    dropped_total: Arc<AtomicU64>,
}

impl StateDeltaActor {
    fn new(capacity: usize, dropped_total: Arc<AtomicU64>) -> Self {
        Self {
            capacity,
            in_flight: Arc::new(AtomicU64::new(0)),
            processed_total: Arc::new(AtomicU64::new(0)),
            dropped_total,
        }
    }

    fn log_summary(&self) {
        let processed = self.processed_total.load(Ordering::Relaxed);
        let dropped = self.dropped_total.load(Ordering::Relaxed);
        let in_flight = self.in_flight.load(Ordering::Relaxed);
        info!(
            processed_total = processed,
            dropped_total = dropped,
            in_flight,
            "StateDelta actor summary"
        );
    }
}

impl Actor for StateDeltaActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(self.capacity);
        info!(
            capacity = self.capacity,
            "StateDelta actor started on dedicated Arbiter"
        );
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
        let in_flight = Arc::clone(&self.in_flight);
        let processed_total = Arc::clone(&self.processed_total);

        let _prev = in_flight.fetch_add(1, Ordering::Relaxed);

        let StateDeltaJob { context, message } = job;
        let context_id = message.context_id;
        let delta_id = message.delta_id;

        let work = async move {
            let started = Instant::now();
            if let Err(err) = handle_state_delta(context, message).await {
                warn!(?err, %context_id, ?delta_id, "Failed to handle state delta");
            } else {
                debug!(
                    %context_id,
                    ?delta_id,
                    elapsed_ms = started.elapsed().as_millis(),
                    "StateDelta worker completed"
                );
            }
        };

        let _spawn_handle = ctx.spawn(work.into_actor(self).map(move |(), _act, _ctx| {
            let _prev = processed_total.fetch_add(1, Ordering::Relaxed);
            let _prev = in_flight.fetch_sub(1, Ordering::Relaxed);
        }));
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
pub fn start_state_delta_actor(
    arbiter: &ArbiterHandle,
    capacity: usize,
) -> StateDeltaSender {
    let dropped_total = Arc::new(AtomicU64::new(0));
    let dropped_for_actor = Arc::clone(&dropped_total);

    let addr = StateDeltaActor::start_in_arbiter(arbiter, move |_ctx| {
        StateDeltaActor::new(capacity, dropped_for_actor)
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

    // Functional tests of `handle_state_delta` itself live in the
    // existing `crates/node/src/handlers/state_delta/mod.rs::tests`
    // module and in the kv-store-with-handlers fuzzy load test (issue
    // #2299 acceptance criteria). The bridge's contract is "delivers
    // the job to a dedicated Arbiter with bounded mailbox" — Actix's
    // own test suite covers `set_mailbox_capacity` and `try_send`.
}
