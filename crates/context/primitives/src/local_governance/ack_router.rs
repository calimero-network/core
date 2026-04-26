//! Concurrent Ack-routing primitive shared between the publish-side
//! `governance_broadcast` helper (calimero-context) and the receive-side
//! gossipsub handler (calimero-node).
//!
//! Lives in `calimero-context-client` so both crates can hold an
//! `Arc<AckRouter>` clone without crossing actor mailboxes — acks land
//! on the gossip path and need to be routed synchronously to the
//! awaiting publisher.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::broadcast;

use super::SignedAck;

/// Routes incoming `SignedAck` messages to in-flight
/// `publish_and_await_ack` callers, keyed by `op_hash`.
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

    /// Count of distinct in-flight `op_hash`es currently subscribed.
    /// Test-only — production code should not depend on this number.
    #[cfg(test)]
    pub fn entry_count(&self) -> usize {
        self.inner.lock().expect("ack_router lock").len()
    }
}

#[cfg(test)]
mod tests {
    use calimero_primitives::identity::PrivateKey;

    use super::*;

    fn dummy_ack(op_hash: [u8; 32]) -> SignedAck {
        SignedAck {
            op_hash,
            signer_pubkey: PrivateKey::random(&mut rand::thread_rng()).public_key(),
            signature: [0u8; 64],
        }
    }

    #[tokio::test]
    async fn ack_router_subscribe_then_route_delivers() {
        let router = AckRouter::default();
        let mut rx = router.subscribe([1u8; 32]);
        let routed = router.route(dummy_ack([1u8; 32]));
        assert!(routed);
        let got = rx.recv().await.expect("ack received");
        assert_eq!(got.op_hash, [1u8; 32]);
    }

    #[tokio::test]
    async fn ack_router_route_with_no_subscriber_returns_false() {
        let router = AckRouter::default();
        let routed = router.route(dummy_ack([2u8; 32]));
        assert!(!routed);
    }

    #[tokio::test]
    async fn ack_router_release_drops_empty_entry() {
        let router = AckRouter::default();
        let rx = router.subscribe([3u8; 32]);
        router.release([3u8; 32], rx);
        assert_eq!(router.entry_count(), 0);
    }

    #[tokio::test]
    async fn ack_router_release_keeps_entry_when_other_receivers_alive() {
        // A second concurrent publish for the same op_hash must keep
        // its subscription alive after the first one releases.
        let router = AckRouter::default();
        let rx_a = router.subscribe([4u8; 32]);
        let _rx_b = router.subscribe([4u8; 32]);
        router.release([4u8; 32], rx_a);
        assert_eq!(
            router.entry_count(),
            1,
            "entry must survive while another receiver is alive"
        );
    }

    #[tokio::test]
    async fn ack_router_release_does_not_leak_when_caller_holds_rx() {
        // Regression: previously a `release(op_hash)` that did not
        // consume `rx` checked `receiver_count() == 0` while the
        // caller's `rx` was still on the stack, leaking one map entry
        // per publish. The current signature consumes `rx`,
        // eliminating the leak.
        let router = AckRouter::default();
        for i in 0..16u8 {
            let key = [i; 32];
            let rx = router.subscribe(key);
            router.release(key, rx);
        }
        assert_eq!(
            router.entry_count(),
            0,
            "release must reap every entry; previously this map would have grown to 16"
        );
    }
}
