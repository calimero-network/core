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
use std::time::{Duration, Instant};

use async_trait::async_trait;
use calimero_context_client::local_governance::{
    hash_scoped_namespace, NamespaceTopicMsg, SignedAck, SignedNamespaceOp,
};
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use libp2p::gossipsub::TopicHash;
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::time::timeout;

use crate::group_store::namespace_member_pubkeys;

#[cfg(test)]
mod tests;

/// Typed-outcome errors returned by the governance broadcast contract.
///
/// `NamespaceNotReady` is the Phase-1 (transport readiness) error;
/// `NoAckReceived` is Phase-2 (ack collection); `Publish`/`LocalApply`
/// wrap underlying failures so callers can match-by-cause.
#[derive(Debug, Error)]
pub enum GovernanceBroadcastError {
    #[error("namespace not ready: mesh={mesh}, required={required}")]
    NamespaceNotReady { mesh: usize, required: usize },
    #[error(
        "no ack received within {waited_ms}ms (op_hash={})",
        hex::encode(op_hash)
    )]
    NoAckReceived { waited_ms: u64, op_hash: [u8; 32] },
    #[error("publish error: {0}")]
    Publish(String),
    #[error("local apply error: {0}")]
    LocalApply(String),
}

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

/// Phase-1 transport-readiness gate: passes iff the gossipsub mesh has
/// at least `min(mesh_n_low, known_subscribers)` peers visible.
///
/// The min cap by `known_subscribers` is what makes a solo-namespace
/// publish succeed: with no known subscribers, `required` is zero and
/// the publish proceeds even with an empty mesh. It also makes a
/// 2-node namespace not block on the full `mesh_n_low` quorum (e.g. 4)
/// it can never reach.
///
/// Pure function — `mesh` and `known_subscribers` are provided by the
/// caller (typically via `NodeClient::mesh_peer_count_for_namespace`
/// and `NodeClient::known_subscribers_for_namespace`). Phase 3.4 wires
/// those plumbing pieces; this function is the policy.
pub fn assert_transport_ready(
    mesh: usize,
    known_subscribers: usize,
    mesh_n_low: usize,
) -> Result<(), GovernanceBroadcastError> {
    let required = std::cmp::min(mesh_n_low, known_subscribers);
    if mesh < required {
        return Err(GovernanceBroadcastError::NamespaceNotReady { mesh, required });
    }
    Ok(())
}

/// Phase-3 typed-outcome on a successful publish: the originator's view
/// of who acked the op and how long it took (start of publish to `min_acks`-th
/// distinct valid signer).
#[derive(Debug, Clone)]
pub struct DeliveryReport {
    pub op_hash: [u8; 32],
    pub acked_by: Vec<PublicKey>,
    pub elapsed_ms: u64,
}

/// Abstraction over the gossipsub transport used by
/// [`publish_and_await_ack_namespace`]. The blanket impl on
/// `NetworkClient` covers production callers; unit tests substitute a
/// stub so they don't need a live actor system.
#[async_trait]
pub trait BroadcastTransport: Send + Sync {
    async fn mesh_peer_count(&self, topic: TopicHash) -> usize;
    async fn publish(&self, topic: TopicHash, bytes: Vec<u8>) -> Result<(), String>;
}

#[async_trait]
impl BroadcastTransport for calimero_network_primitives::client::NetworkClient {
    async fn mesh_peer_count(&self, topic: TopicHash) -> usize {
        Self::mesh_peer_count(self, topic).await
    }
    async fn publish(&self, topic: TopicHash, bytes: Vec<u8>) -> Result<(), String> {
        Self::publish(self, topic, bytes)
            .await
            .map(|_msg_id| ())
            .map_err(|e| e.to_string())
    }
}

/// Publish a namespace governance op and collect signed acks until
/// `min_acks` distinct valid signers have acked or the deadline passes.
///
/// **Phase-1 readiness is the caller's responsibility.** Phase 3.4 keeps
/// `publish_and_await_ack_namespace` as the Phase-2/3 (publish + collect +
/// outcome) primitive; gating on `assert_transport_ready` happens at the
/// caller (typically `NamespaceGovernance::sign_apply_and_publish` once
/// Phase 5 wires it). This split lets the helper be unit-testable
/// without dragging mesh/subscriber state into every test.
///
/// Behavior:
///   * Subscribes to the per-`op_hash` ack channel **before** publishing
///     so a fast ack from a peer that already had the op cannot race
///     past the subscription.
///   * Drops acks failing [`verify_ack`] (wrong op_hash, bad signature,
///     non-member signer) silently — they're best-effort gossip.
///   * Filters by `required_signers` if provided (e.g. KeyDelivery
///     requires the recipient's ack specifically).
///   * Dedups by `signer_pubkey` so duplicate gossip rebroadcasts of the
///     same ack don't inflate `acked_by`.
///   * Returns `Ok(DeliveryReport)` once `acked_by.len() >= min_acks`,
///     `Err(NoAckReceived)` on timeout or channel close.
pub async fn publish_and_await_ack_namespace(
    store: &Store,
    transport: &dyn BroadcastTransport,
    ack_router: &AckRouter,
    namespace_id: [u8; 32],
    topic: TopicHash,
    op: SignedNamespaceOp,
    op_timeout: Duration,
    min_acks: usize,
    required_signers: Option<Vec<PublicKey>>,
) -> Result<DeliveryReport, GovernanceBroadcastError> {
    let topic_id = topic.as_str().as_bytes();
    let op_hash = hash_scoped_namespace(topic_id, &op)
        .map_err(|e| GovernanceBroadcastError::Publish(e.to_string()))?;
    let start = Instant::now();

    // Subscribe BEFORE publishing so a peer that already has this op
    // (e.g. via concurrent backfill) cannot ack faster than our
    // subscription registration and have its ack dropped.
    let mut rx = ack_router.subscribe(op_hash);
    let payload = borsh::to_vec(&NamespaceTopicMsg::Op(op))
        .map_err(|e| GovernanceBroadcastError::Publish(e.to_string()))?;
    transport
        .publish(topic, payload)
        .await
        .map_err(GovernanceBroadcastError::Publish)?;

    let mut acked_by: Vec<PublicKey> = Vec::new();
    let deadline = start + op_timeout;
    loop {
        // saturating_duration_since returns ZERO past the deadline (no
        // Instant subtraction panic) — `tokio::time::timeout` then
        // resolves immediately as `Err(_elapsed)` on the zero duration.
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            ack_router.release(op_hash, rx);
            return Err(GovernanceBroadcastError::NoAckReceived {
                waited_ms: start.elapsed().as_millis() as u64,
                op_hash,
            });
        }
        match timeout(remaining, rx.recv()).await {
            Ok(Ok(ack)) => {
                if !verify_ack(store, namespace_id, op_hash, &ack) {
                    continue;
                }
                if let Some(req) = &required_signers {
                    if !req.contains(&ack.signer_pubkey) {
                        continue;
                    }
                }
                if !acked_by.iter().any(|p| *p == ack.signer_pubkey) {
                    acked_by.push(ack.signer_pubkey);
                }
                if acked_by.len() >= min_acks {
                    ack_router.release(op_hash, rx);
                    return Ok(DeliveryReport {
                        op_hash,
                        acked_by,
                        elapsed_ms: start.elapsed().as_millis() as u64,
                    });
                }
            }
            // Lagged(n): we missed n messages but the channel is still
            // open — keep polling. n is bounded by broadcast capacity (64).
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            // Closed: all senders dropped (typically because a concurrent
            // flow released the AckRouter entry as the last subscriber).
            // `recv()` would return immediately on every subsequent call —
            // `continue` would burn CPU until the deadline. Treat as terminal.
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                ack_router.release(op_hash, rx);
                return Err(GovernanceBroadcastError::NoAckReceived {
                    waited_ms: start.elapsed().as_millis() as u64,
                    op_hash,
                });
            }
            Err(_elapsed) => {
                ack_router.release(op_hash, rx);
                return Err(GovernanceBroadcastError::NoAckReceived {
                    waited_ms: start.elapsed().as_millis() as u64,
                    op_hash,
                });
            }
        }
    }
}
