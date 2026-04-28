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

use std::time::{Duration, Instant};

use async_trait::async_trait;
use calimero_context_client::local_governance::{
    hash_scoped_namespace, AckRouter, GovernanceError, NamespaceOp, NamespaceTopicMsg, RootOp,
    SignedAck, SignedNamespaceOp, SignedReadinessBeacon,
};
use calimero_node_primitives::sync::{BroadcastMessage, MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use libp2p::gossipsub::{PublishError, TopicHash};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::time::timeout;

use crate::group_store::namespace_member_pubkeys;

/// Default `min_acks` for governance publishes — at least one peer must
/// ack before we consider the op delivered. Spec §6.2. Callers that
/// need a stricter quorum (e.g. KeyDelivery requiring the recipient's
/// ack) override per-call.
pub const DEFAULT_MIN_ACKS: usize = 1;

#[derive(Debug, Clone, Error)]
pub enum BroadcastPublishError {
    #[error("no peers subscribed to topic")]
    NoPeersSubscribed,
    #[error("{0}")]
    Other(String),
}

pub(crate) fn classify_network_publish_error(e: eyre::Report) -> BroadcastPublishError {
    let no_peers = e.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<PublishError>(),
            Some(PublishError::NoPeersSubscribedToTopic)
        )
    });
    if no_peers {
        BroadcastPublishError::NoPeersSubscribed
    } else {
        BroadcastPublishError::Other(e.to_string())
    }
}

/// Compute the gossipsub topic for a namespace governance publish.
/// Mirrors the format used by `NodeClient::publish_signed_namespace_op`
/// and the receiver-side `network_event::namespace` handler.
#[must_use]
pub fn ns_topic(namespace_id: [u8; 32]) -> TopicHash {
    TopicHash::from_raw(format!("ns/{}", hex::encode(namespace_id)))
}

/// Per-op publish timeout for "cheap" governance ops — alias / metadata
/// writes whose apply path is O(1) on every receiver. The 2s budget
/// comfortably covers a fresh GRAFT (≤1s) plus one round-trip ack.
pub const OP_ACK_CHEAP_TIMEOUT: Duration = Duration::from_secs(2);

/// Per-op publish timeout for member-change governance ops — add /
/// remove members, MemberJoined, capability flips. Receivers walk
/// inheritance edges and may rotate group keys, so a 5s budget is
/// realistic on top of the cheap baseline.
pub const OP_ACK_MEMBER_CHANGE_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-op publish timeout for "heavy" governance ops — context
/// creation, app installation, namespace bootstrap. Receivers may
/// unwrap large envelopes, store stub application metadata, and
/// trigger downstream retry loops; 10s mirrors the snapshot-sync
/// class.
pub const OP_ACK_HEAVY_TIMEOUT: Duration = Duration::from_secs(10);

/// Pick the appropriate per-op timeout for a `NamespaceOp` based on the
/// receiver-side apply work it implies. The classification is:
///
///   * `AdminChanged` / `PolicyUpdated`: cheap — single-row writes.
///   * `MemberJoined` / `GroupCreated` / `GroupReparented`: member-change —
///     membership-table mutations, possible inheritance walks.
///   * `GroupDeleted` / `KeyDelivery`: heavy — cascade deletes touch every
///     descendant subtree row; key delivery may unwrap large envelopes.
///   * `Group { encrypted, .. }`: member-change baseline. The inner
///     `GroupOp` variant isn't visible without decrypting, so a finer
///     classification (e.g. `KeyDelivery` heavy via the rotation
///     envelope) requires accepting a wider tail. Member-change is the
///     conservative middle ground.
#[must_use]
pub fn timeout_for_namespace_op(op: &NamespaceOp) -> Duration {
    match op {
        NamespaceOp::Root(RootOp::AdminChanged { .. } | RootOp::PolicyUpdated { .. }) => {
            OP_ACK_CHEAP_TIMEOUT
        }
        NamespaceOp::Root(
            RootOp::MemberJoined { .. }
            | RootOp::GroupCreated { .. }
            | RootOp::GroupReparented { .. },
        ) => OP_ACK_MEMBER_CHANGE_TIMEOUT,
        NamespaceOp::Root(RootOp::GroupDeleted { .. } | RootOp::KeyDelivery { .. }) => {
            OP_ACK_HEAVY_TIMEOUT
        }
        NamespaceOp::Group { .. } => OP_ACK_MEMBER_CHANGE_TIMEOUT,
    }
}

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

/// Verify a [`SignedReadinessBeacon`] against the local store's view of
/// namespace membership.
///
/// The Ed25519 signature is checked via
/// [`SignedReadinessBeacon::verify_signature`], which uses the canonical
/// `READINESS_BEACON_SIGN_DOMAIN || borsh(SignableReadinessBeacon)`
/// payload defined alongside the wire type in
/// `calimero_context_client::local_governance::wire`. This rejects
/// field-substitution replays (proven by the `signed_readiness_beacon_*`
/// tamper tests in that module).
///
/// The membership check uses [`namespace_member_pubkeys`], which
/// includes the meta admin even when the admin has no member row —
/// matching `verify_ack`'s behaviour and ensuring legitimate beacons
/// from the namespace creator are not silently dropped.
///
/// Returns `false` on any failure (signature, membership, store error)
/// so the receiver can drop the beacon without surfacing an error to
/// the gossipsub layer.
#[must_use]
pub fn verify_readiness_beacon(store: &Store, beacon: &SignedReadinessBeacon) -> bool {
    if beacon.verify_signature().is_err() {
        return false;
    }
    namespace_member_pubkeys(store, beacon.namespace_id)
        .map(|members| members.contains(&beacon.peer_pubkey))
        .unwrap_or(false)
}

/// Sign an ack for `op_hash` using `signer_sk`.
///
/// The Ed25519 signature covers
/// [`SignedAck::signable_bytes`](calimero_context_client::local_governance::SignedAck::signable_bytes),
/// i.e. `ACK_SIGN_DOMAIN || op_hash` — domain-separated to prevent
/// substituting a signature taken over the same 32-byte hash on a
/// different protocol surface.
///
/// Returns `Err` only if the underlying signer rejects the message
/// (extremely unlikely for a well-formed `PrivateKey`); callers in the
/// receiver path log and drop on error so a single bad ack never stops
/// op apply.
pub fn sign_ack(signer_sk: &PrivateKey, op_hash: [u8; 32]) -> Result<SignedAck, GovernanceError> {
    let msg = SignedAck::signable_bytes(&op_hash);
    let signature = signer_sk.sign(&msg)?.to_bytes();
    Ok(SignedAck {
        op_hash,
        signer_pubkey: signer_sk.public_key(),
        signature,
    })
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
/// and `NodeClient::known_subscribers`). This function is the policy.
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

/// Per-handler delivery observability log.
///
/// The publish-boundary helpers
/// (`NamespaceGovernance::sign_apply_and_publish` and friends) already
/// log a `tracing::debug!` line carrying op_kind / acks / elapsed_ms —
/// good enough for protocol-level diagnostics but invisible at the
/// default `info` log level. Handlers in `crates/context/src/handlers/*`
/// own the user-facing API surface, so promoting an `info!` line here
/// gives operators a per-endpoint signal: which API call flaked, on
/// which namespace, and whether the ack collection landed within the
/// op timeout. Centralising the log shape keeps every handler emit
/// identical so log-aggregation queries don't have to special-case the
/// per-endpoint format.
///
/// `acked_by.is_empty()` is logged at `warn!` to highlight cold-start
/// or GRAFT-window publishes that landed on the wire but didn't gather
/// confirmation in time. The local DAG advance is durable in either
/// case — the warn is a hint that downstream peers may need
/// reconciliation, not a failure indication.
pub fn observe_handler_delivery(handler: &str, op_kind: &str, report: &DeliveryReport) {
    let outcome = if report.acked_by.is_empty() {
        "empty"
    } else {
        "acked"
    };
    crate::metrics::record_governance_handler_delivery(
        handler,
        op_kind,
        outcome,
        report.elapsed_ms,
    );
    if report.acked_by.is_empty() {
        tracing::warn!(
            handler,
            op_kind,
            elapsed_ms = report.elapsed_ms,
            op_hash = %hex::encode(report.op_hash),
            "governance op published but no acks collected before deadline \
             (publisher boundary deemed delivery best-effort; receivers will \
             reconcile via heartbeat)"
        );
    } else {
        tracing::info!(
            handler,
            op_kind,
            acks = report.acked_by.len(),
            elapsed_ms = report.elapsed_ms,
            op_hash = %hex::encode(report.op_hash),
            "governance op delivered"
        );
    }
}

/// Abstraction over the gossipsub transport used by
/// [`publish_and_await_ack_namespace`]. The blanket impl on
/// `NetworkClient` covers production callers; unit tests substitute a
/// stub so they don't need a live actor system.
#[async_trait]
pub trait BroadcastTransport: Send + Sync {
    async fn mesh_peer_count(&self, topic: TopicHash) -> usize;
    async fn publish(&self, topic: TopicHash, bytes: Vec<u8>) -> Result<(), BroadcastPublishError>;
}

#[async_trait]
impl BroadcastTransport for calimero_network_primitives::client::NetworkClient {
    async fn mesh_peer_count(&self, topic: TopicHash) -> usize {
        Self::mesh_peer_count(self, topic).await
    }
    async fn publish(&self, topic: TopicHash, bytes: Vec<u8>) -> Result<(), BroadcastPublishError> {
        Self::publish(self, topic, bytes)
            .await
            .map(|_msg_id| ())
            .map_err(classify_network_publish_error)
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
///
/// Contract — eventually-consistent, best-effort:
///
/// `Ok` means "publish accepted by gossipsub". The op is on the wire,
/// receivers will apply + ack as they catch up. The exact ack count
/// depends on mesh state and receiver-side backfill latency:
///
///   * `Ok` with `acked_by.len() >= min_acks` — enough distinct
///     signers acked within `op_timeout`. Tight happy path.
///   * `Ok` with `acked_by.len() < min_acks` (incl. empty) —
///     publish accepted but acks didn't arrive in time. Common during
///     a cold-state join window where the receiver needs to backfill
///     missing parents (`apply` only emits the ack post-apply). The
///     caller's local apply (if any) is durable; surfacing a partial
///     report rather than `Err` keeps the caller from retrying an
///     already-applied op and double-advancing the local DAG.
///   * `Ok` with empty `acked_by` is also the solo-namespace fast path
///     (`min_acks == 0`).
///
/// `Err` only when the publish itself never delivered:
///
///   * `Err(Publish)` — gossipsub rejected the message (signing,
///     oversize, transform error).
///   * `Err(NoAckReceived)` — the transport reported no subscribed peers
///     while `min_acks > 0` (no delivery was attempted; surfacing `Ok`
///     would be a lie).
///   * `Err(NamespaceNotReady)` — not raised by this fn; readiness
///     gating is the caller's responsibility.
///
/// Phase 5 callers treat the publisher-boundary `tracing::debug!`
/// (op_kind + acks + elapsed_ms) as the source of truth for delivery
/// observability and proceed regardless of `acked_by.len()` — the
/// local DAG advance is the durable side-effect, the ack is just
/// confirmation that some peer received it before the deadline.
/// Phases 8/9 will introduce a strict-confirmation path (KeyDelivery
/// recipient ack must arrive before `join_group` returns) by passing
/// the recipient's pubkey as `required_signers` and inspecting
/// `report.acked_by` at the call site — at that point the helper to
/// turn a partial report into `Err` will be added alongside the first
/// caller that needs it.
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

    // Wire framing on `ns/<id>` is two-layer: the gossipsub frame
    // decodes as `BroadcastMessage` first (see
    // `node/src/handlers/network_event.rs::handle_network_event`),
    // and only the `NamespaceGovernanceDelta::payload` field is then
    // decoded as `NamespaceTopicMsg`. Publishing the inner enum raw
    // would deserialize-fail at the receiver and be silently dropped
    // before reaching the `Op` arm — which would make every Phase-5
    // caller of this helper observe `NoAckReceived` regardless of
    // mesh health. Mirrors `client.rs::publish_signed_namespace_op`
    // and the heartbeat-republish site at `namespace.rs:223`.
    //
    // All synchronous serialization happens BEFORE `ack_router.subscribe`
    // so a borsh / size-check failure cannot leak a channel registration:
    // we only enter the wait state once the publish has actually been
    // handed to the gossipsub layer. This keeps the `subscribe-before-
    // publish` race-free guarantee documented above intact (`subscribe`
    // still happens before the wire `publish` call).
    let delta_id = op
        .content_hash()
        .map_err(|e| GovernanceBroadcastError::Publish(format!("content_hash: {e}")))?;
    let parent_ids = op.parent_op_hashes.clone();
    let inner = borsh::to_vec(&NamespaceTopicMsg::Op(op))
        .map_err(|e| GovernanceBroadcastError::Publish(e.to_string()))?;
    // `MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES` caps the inner
    // `borsh(NamespaceTopicMsg::Op(SignedNamespaceOp))` bytes — the
    // value the constant's doc at `sync/snapshot.rs` and the legacy
    // `NodeClient::publish_signed_namespace_op` (which checks
    // `signed_op_borsh.len()`) both describe. Earlier in this PR the
    // check ran on the outer `BroadcastMessage` envelope bytes (~100B
    // overhead from the variant tag + 32B namespace_id + 32B delta_id
    // + length-prefixed parent_ids), creating split-enforcement
    // asymmetry: a borderline 64KB inner op accepted by the legacy
    // path would be rejected here. Checking the inner keeps both
    // paths consistent and matches the constant's documented intent.
    if inner.len() > MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES {
        return Err(GovernanceBroadcastError::Publish(format!(
            "signed namespace op payload exceeds max ({} > {})",
            inner.len(),
            MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES
        )));
    }
    let envelope = BroadcastMessage::NamespaceGovernanceDelta {
        namespace_id,
        delta_id,
        parent_ids,
        payload: inner,
    };
    let payload =
        borsh::to_vec(&envelope).map_err(|e| GovernanceBroadcastError::Publish(e.to_string()))?;

    let start = Instant::now();
    // Subscribe BEFORE publishing so a peer that already has this op
    // (e.g. via concurrent backfill) cannot ack faster than our
    // subscription registration and have its ack dropped.
    let mut rx = ack_router.subscribe(op_hash);
    match transport.publish(topic, payload).await {
        Ok(()) => {}
        // The transport boundary classifies libp2p's
        // `PublishError::NoPeersSubscribedToTopic` while the concrete
        // error type is still available. The intended behaviour depends
        // on `min_acks`:
        //
        //   * `min_acks == 0` — caller opted out of confirmation, so
        //     "delivered to no one" is a legitimate Ok-with-empty result.
        //     Solo namespaces (`assert_transport_ready` passed because
        //     `known_subscribers == 0`) compute this min_acks and reach
        //     this arm. Matches the legacy
        //     `NodeClient::publish_signed_namespace_op` semantics that
        //     warned and returned Ok rather than propagating.
        //
        //   * `min_acks > 0` — caller asked for confirmation, and zero
        //     mesh peers means it cannot be obtained. Surfacing this as
        //     `Ok(DeliveryReport { acked_by: [] })` would silently lie
        //     about the contract; instead return `NoAckReceived` so the
        //     caller can decide to retry, escalate, or fall through to a
        //     best-effort path explicitly.
        //
        // Hard publish failures (signing errors, oversized messages,
        // transform errors) still propagate as `Publish` regardless of
        // `min_acks`.
        Err(BroadcastPublishError::NoPeersSubscribed) => {
            ack_router.release(op_hash, rx);
            if min_acks == 0 {
                return Ok(DeliveryReport {
                    op_hash,
                    acked_by: Vec::new(),
                    elapsed_ms: start.elapsed().as_millis() as u64,
                });
            }
            return Err(GovernanceBroadcastError::NoAckReceived {
                waited_ms: start.elapsed().as_millis() as u64,
                op_hash,
            });
        }
        Err(e) => {
            // Other publish failures: release the channel registration
            // before propagating, otherwise the entry stays subscribed
            // forever (op_hash is single-use, so no future caller reuses
            // it).
            ack_router.release(op_hash, rx);
            return Err(GovernanceBroadcastError::Publish(e.to_string()));
        }
    }

    // `min_acks == 0` means "publish-only, don't wait" — the expected
    // outcome is immediate `Ok`, not a `NoAckReceived` after `op_timeout`
    // elapses. The collect loop's threshold check sits inside the
    // `Ok(Ok(ack))` arm and is therefore unreachable when no ack ever
    // arrives, so without this short-circuit the function would block
    // for the full `op_timeout` and then surface the wrong error.
    // Spec §6.2 documents the default as `1`; this guard also makes the
    // primitive well-behaved if a future caller (e.g. a "broadcast and
    // forget" variant) opts out of waiting.
    if min_acks == 0 {
        ack_router.release(op_hash, rx);
        return Ok(DeliveryReport {
            op_hash,
            acked_by: Vec::new(),
            elapsed_ms: start.elapsed().as_millis() as u64,
        });
    }

    let mut acked_by: Vec<PublicKey> = Vec::new();
    let deadline = start + op_timeout;
    // Helper: build a `DeliveryReport` for whatever acks we did manage
    // to collect. Used at every post-publish exit (deadline elapsed,
    // channel closed). Once the publish succeeded the op is on the
    // wire — failing the call here would falsely suggest non-delivery
    // and tempt the caller to retry, which (for the apply-then-publish
    // flow) doubles the local DAG advance and creates the orphan-op
    // pattern the gate exists to prevent. Surface the partial state as
    // `Ok` and let callers compare `acked_by.len()` against their own
    // confirmation expectations.
    let report = |acked_by: Vec<PublicKey>, start: Instant| DeliveryReport {
        op_hash,
        acked_by,
        elapsed_ms: start.elapsed().as_millis() as u64,
    };
    loop {
        // saturating_duration_since returns ZERO past the deadline (no
        // Instant subtraction panic) — `tokio::time::timeout` then
        // resolves immediately as `Err(_elapsed)` on the zero duration.
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            ack_router.release(op_hash, rx);
            // Deadline exceeded after a successful publish. The op is
            // gossiped; receivers may still be backfilling missing
            // parents (cold-state join window) and will apply + ack
            // later. Return what we collected — the caller decides
            // whether `acked_by.len()` meets their needs.
            return Ok(report(acked_by, start));
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
                    return Ok(report(acked_by, start));
                }
            }
            // Lagged(n): we missed n messages but the channel is still
            // open — keep polling. n is bounded by broadcast capacity (64).
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            // Closed: all senders dropped (typically because a concurrent
            // flow released the AckRouter entry as the last subscriber).
            // `recv()` would return immediately on every subsequent call —
            // `continue` would burn CPU until the deadline. Treat as
            // terminal but, like the deadline arm, return `Ok` with the
            // partial `acked_by` rather than `Err` — the publish itself
            // succeeded and the caller's local apply (if any) must not
            // be invalidated by router GC.
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                ack_router.release(op_hash, rx);
                return Ok(report(acked_by, start));
            }
            Err(_elapsed) => {
                ack_router.release(op_hash, rx);
                return Ok(report(acked_by, start));
            }
        }
    }
}
