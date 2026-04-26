//! Acked-broadcast helper for governance / KeyDelivery publishes.
//!
//! Phase 3 of the three-phase governance contract for #2237. This module
//! lands the central choke-point that future `sign_and_publish_*` paths
//! will delegate to. Contains:
//!
//!   * [`AckRouter`] — routes incoming `SignedAck` messages from the wire
//!     receiver to the in-flight publisher waiting on a specific op_hash.
//!
//! Tasks 3.2 — 3.4 add `verify_ack`, `assert_transport_ready`, and
//! `publish_and_await_ack` on top of this skeleton.

use std::collections::HashMap;
use std::sync::Mutex;

use calimero_context_client::local_governance::SignedAck;
use tokio::sync::broadcast;

#[cfg(test)]
mod tests;

/// Routes incoming Ack messages to in-flight `publish_and_await_ack`
/// callers, keyed by `op_hash`.
///
/// Each in-flight publish [`subscribe`](Self::subscribe)s to the per-op
/// channel before publishing and [`release`](Self::release)s the
/// receiver on completion (Ok or NoAck), at which point the entry is
/// reaped if no other concurrent publish is waiting on the same op.
#[derive(Debug, Default)]
pub struct AckRouter {
    inner: Mutex<HashMap<[u8; 32], broadcast::Sender<SignedAck>>>,
}

impl AckRouter {
    /// Register interest in acks for `op_hash`. Returns a receiver that
    /// fires once per ack the wire layer routes here.
    pub fn subscribe(&self, op_hash: [u8; 32]) -> broadcast::Receiver<SignedAck> {
        let mut g = self.inner.lock().expect("ack_router lock");
        let tx = g
            .entry(op_hash)
            .or_insert_with(|| broadcast::channel(64).0)
            .clone();
        tx.subscribe()
    }

    /// Called from the wire receiver's `Ack` arm. Returns `true` if any
    /// subscriber was registered for the op (purely for telemetry — not
    /// load-bearing for correctness).
    pub fn route(&self, ack: SignedAck) -> bool {
        let g = self.inner.lock().expect("ack_router lock");
        match g.get(&ack.op_hash) {
            Some(tx) => tx.send(ack).is_ok(),
            None => false,
        }
    }

    /// Called when a publish completes (Ok or NoAck) to GC entries with
    /// no remaining receivers. **Consumes the receiver** so it is dropped
    /// before we inspect `receiver_count()`; otherwise the caller's
    /// still-live `rx` on the stack would keep the count ≥ 1 and the
    /// entry would never be reaped, leaking one map entry per publish.
    /// Idempotent — safe to call multiple times.
    pub fn release(&self, op_hash: [u8; 32], rx: broadcast::Receiver<SignedAck>) {
        drop(rx);
        let mut g = self.inner.lock().expect("ack_router lock");
        if let Some(tx) = g.get(&op_hash) {
            if tx.receiver_count() == 0 {
                let _ = g.remove(&op_hash);
            }
        }
    }
}
