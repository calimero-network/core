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
use calimero_store::Store;
use tokio::sync::broadcast;

use crate::group_store::namespace_member_pubkeys;

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

/// Validate an incoming `SignedAck` for a publish in flight.
///
/// Three checks, all silent on failure (the caller should drop the ack
/// rather than propagate an error — acks are best-effort gossip):
///
/// 1. `ack.op_hash` matches the `expected_op_hash` the publisher is
///    waiting on (topic-scoped via `hash_scoped_namespace` /
///    `hash_scoped_group`, so cross-topic replays are already excluded
///    at hash construction time).
/// 2. `ack.verify_signature()` succeeds — Ed25519 over
///    [`SignedAck::signable_bytes`], i.e. `ACK_SIGN_DOMAIN || op_hash`.
///    The domain prefix is what stops an attacker from substituting a
///    signature taken over the same 32-byte hash on a different
///    protocol surface.
/// 3. `ack.signer_pubkey` is a current member of `namespace_id` at
///    this node's local DAG view — non-members cannot ack.
pub fn verify_ack(
    store: &Store,
    namespace_id: [u8; 32],
    expected_op_hash: [u8; 32],
    ack: &SignedAck,
) -> bool {
    if ack.op_hash != expected_op_hash {
        return false;
    }
    if ack.verify_signature().is_err() {
        return false;
    }
    namespace_member_pubkeys(store, namespace_id)
        .map(|members| members.contains(&ack.signer_pubkey))
        .unwrap_or(false)
}
