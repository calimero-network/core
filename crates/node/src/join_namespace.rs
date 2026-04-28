//! J6 namespace-join flow: split `join_namespace` into a fast probe-then-
//! beacon path and a separate `await_namespace_ready` that runs backfill
//! and publishes `RootOp::MemberJoined` through the three-phase contract.
//!
//! Phase 8 of the three-phase governance contract (#2237).
//!
//! These functions live as free functions in `calimero-node` (rather
//! than as methods on `ContextClient` per the plan's `&self` shape)
//! because `ContextClient` lives in `calimero-context-primitives`,
//! which cannot depend on `calimero-node` (where `ReadinessCache` and
//! `ReadinessCacheNotify` live) without a Cargo cycle. The
//! free-function form takes the cache/notify as explicit args and is
//! callable from any holder of those Arcs (server handlers, tests,
//! [`NodeManager`] internals).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use calimero_context::governance_broadcast::ns_topic;
use calimero_context::group_store::{self, namespace_member_pubkeys, NamespaceGovernance};
use calimero_context_client::local_governance::{
    AckRouter, NamespaceOp, NamespaceTopicMsg, ReadinessProbe, RootOp,
};
use calimero_context_config::types::{ContextGroupId, SignedGroupOpenInvitation};
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use rand::Rng;
use thiserror::Error;
use tracing::debug;

use crate::readiness::{ReadinessCache, ReadinessCacheNotify, ReadinessConfig};

/// Outcome of `join_namespace` (J6 fast path).
#[derive(Debug, Clone)]
pub struct JoinStarted {
    pub namespace_id: [u8; 32],
    pub sync_partner: PublicKey,
    pub partner_head: [u8; 32],
    pub partner_applied: u64,
    pub elapsed_ms: u64,
}

/// Outcome of `await_namespace_ready` (J6 slow path).
#[derive(Debug, Clone)]
pub struct ReadyReport {
    pub namespace_id: [u8; 32],
    pub final_head: [u8; 32],
    pub applied_through: u64,
    pub members_learned: usize,
    pub acked_by: Vec<PublicKey>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Error)]
pub enum JoinError {
    /// No fresh beacon arrived before the deadline. The most common
    /// trigger is a cold-start joiner whose namespace mesh hasn't seen
    /// any *Ready peers yet — try again with `join_namespace_with_retry`.
    #[error("no ready peers responded within {waited_ms}ms")]
    NoReadyPeers { waited_ms: u64 },
    #[error("invitation invalid: {0}")]
    InvalidInvitation(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("local: {0}")]
    Local(String),
}

#[derive(Debug, Error)]
pub enum ReadyError {
    #[error("no ready peers in cache")]
    NoReadyPeers,
    #[error("backfill: {0}")]
    Backfill(String),
    #[error("publish MemberJoined: {0}")]
    PublishMemberJoined(String),
    #[error("local: {0}")]
    Local(String),
    #[error("join failed: {0}")]
    JoinFailed(String),
    #[error("invitation invalid: {0}")]
    InvalidInvitation(String),
}

/// J6 fast path: provision identity, subscribe to the namespace topic,
/// publish a [`ReadinessProbe`], and resolve on the first fresh beacon.
///
/// Returns [`JoinStarted`] carrying the partner the caller should sync
/// against in [`await_namespace_ready`].
///
/// Implementation steps mirror spec §8.1:
/// 1. Validate the invitation expiration locally — no transport hop
///    needed for an obviously-stale invitation.
/// 2. `get_or_create_namespace_identity` — provisions the joiner's
///    namespace identity if absent. Idempotent: repeat-calls with the
///    same `group_id` reuse the stored identity.
/// 3. `subscribe_namespace` — gossipsub subscription is the precondition
///    for any peer beacon to reach our cache.
/// 4. Publish a `ReadinessProbe` so a *Ready peer can short-circuit the
///    periodic emission interval — closes the cold-start window.
/// 5. `await_first_fresh_beacon` — registers a `Notified` future
///    BEFORE checking the cache (Phase 8.1's race-fix), then resolves
///    on the first beacon-or-already-cached entry, or times out.
///
/// `deadline` is the FULL function budget (steps 1–5). The probe wait
/// budget is `deadline.saturating_sub(start.elapsed())`, so a slow
/// step 1–4 doesn't overshoot the caller's stated total.
pub async fn join_namespace(
    store: &Store,
    node_client: &NodeClient,
    readiness_cache: &Arc<ReadinessCache>,
    readiness_notify: &Arc<ReadinessCacheNotify>,
    config: &ReadinessConfig,
    invitation: SignedGroupOpenInvitation,
    deadline: Duration,
) -> Result<JoinStarted, JoinError> {
    let start = Instant::now();

    // step 1: validate invitation expiration locally.
    let group_id = invitation.invitation.group_id;
    let expiration = invitation.invitation.expiration_timestamp;
    if expiration != 0 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now > expiration {
            return Err(JoinError::InvalidInvitation("invitation expired".into()));
        }
    }

    // step 2: provision identity (mark_membership_pending equivalent —
    // the namespace identity row IS the local pending marker until
    // MemberJoined ack arrives).
    let (ns_id, _pk, _sk_bytes, _sender_key) =
        group_store::get_or_create_namespace_identity(store, &group_id)
            .map_err(|e| JoinError::Local(e.to_string()))?;
    let namespace_id = ns_id.to_bytes();

    // step 3: subscribe to namespace topic. Idempotent.
    node_client
        .subscribe_namespace(namespace_id)
        .await
        .map_err(|e| JoinError::Transport(e.to_string()))?;

    // step 4: publish a ReadinessProbe. Best-effort — a publish error
    // here (e.g. NoPeersSubscribedToTopic on a solo node) is not fatal:
    // the probe simply doesn't reach anyone, and `await_first_fresh_beacon`
    // below will time out with `NoReadyPeers`.
    let probe = ReadinessProbe {
        namespace_id,
        nonce: rand::random(),
    };
    let inner = borsh::to_vec(&NamespaceTopicMsg::ReadinessProbe(probe))
        .map_err(|e| JoinError::Transport(e.to_string()))?;
    // Wrap in the BroadcastMessage envelope used on `ns/<id>` topics —
    // the receiver-side dispatch in `network_event::handle_namespace_governance_delta`
    // unwraps NamespaceGovernanceDelta and decodes the inner
    // NamespaceTopicMsg. delta_id/parent_ids are zero/empty since
    // probes are not DAG content.
    let envelope = BroadcastMessage::NamespaceGovernanceDelta {
        namespace_id,
        delta_id: [0u8; 32],
        parent_ids: Vec::new(),
        payload: inner,
    };
    let bytes = borsh::to_vec(&envelope).map_err(|e| JoinError::Transport(e.to_string()))?;
    let topic = ns_topic(namespace_id);
    if let Err(err) = node_client.network_client().publish(topic, bytes).await {
        debug!(
            ?err,
            "ReadinessProbe publish failed (non-fatal — solo or no-peers)"
        );
    }

    // step 5: collect the first fresh beacon. Clamp the wait to the
    // remaining budget so total wall-clock stays within the caller's
    // `deadline`.
    let remaining = deadline.saturating_sub(start.elapsed());
    let (sync_partner, entry) = readiness_cache
        .await_first_fresh_beacon(
            readiness_notify,
            namespace_id,
            config.ttl_heartbeat,
            remaining,
        )
        .await
        .ok_or_else(|| JoinError::NoReadyPeers {
            waited_ms: start.elapsed().as_millis() as u64,
        })?;

    Ok(JoinStarted {
        namespace_id,
        sync_partner,
        partner_head: entry.head,
        partner_applied: entry.applied_through,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

/// J6 slow path: backfill the namespace DAG against the partner picked
/// in [`join_namespace`], then publish `RootOp::MemberJoined` through
/// the three-phase contract and wait for at least one ack.
///
/// Returns [`ReadyReport`] with the post-publish observable state
/// (final head, applied_through, member count). An empty `acked_by` is
/// surfaced via the underlying `DeliveryReport` when the publish
/// timed out without acks — the caller decides whether that's
/// acceptable for their flow.
///
/// `deadline` is the FULL function budget. Sub-steps are not
/// individually deadline-clamped because the backfill duration is
/// unbounded by the caller's perspective; the
/// `sign_and_publish_namespace_op` call carries its own per-op timeout
/// derived from `op_kind_label`.
pub async fn await_namespace_ready(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    invitation: SignedGroupOpenInvitation,
    namespace_id: [u8; 32],
    deadline: Duration,
) -> Result<ReadyReport, ReadyError> {
    let start = Instant::now();

    // step 1: backfill — request a namespace governance pull. This
    // schedules `SyncManager::sync_namespace` which opens a stream to
    // a mesh peer and applies received ops. We do not block on
    // completion of that stream here — the assumption is that by the
    // time we publish MemberJoined below, our local applied_through
    // has caught up enough to validate the op. A targeted pull from
    // the J6 sync_partner is the future refinement.
    if let Err(err) = node_client.sync_namespace(namespace_id).await {
        debug!(
            ?err,
            "namespace sync request failed (non-fatal — try publish anyway)"
        );
    }

    // Best-effort wait for backfill progress. We sleep up to
    // `min(deadline / 4, 2s)` to give the sync stream time to apply
    // ops before we publish MemberJoined. Without this, the publish
    // can succeed but our local DAG remains behind.
    let backfill_wait = std::cmp::min(deadline / 4, Duration::from_secs(2));
    tokio::time::sleep(backfill_wait).await;

    // step 2: load the namespace identity for signing MemberJoined.
    let group_id = ContextGroupId::from(namespace_id);
    let (_, my_pk, my_sk_bytes, _) =
        group_store::get_or_create_namespace_identity(store, &group_id)
            .map_err(|e| ReadyError::Local(e.to_string()))?;
    let signing_key = PrivateKey::from(my_sk_bytes);

    // step 3: publish MemberJoined via three-phase contract.
    let op = NamespaceOp::Root(RootOp::MemberJoined {
        member: my_pk,
        signed_invitation: invitation,
    });
    let report = NamespaceGovernance::new(store, namespace_id)
        .sign_and_publish_without_apply(node_client, ack_router, &signing_key, op)
        .await
        .map_err(|e| ReadyError::PublishMemberJoined(e.to_string()))?;

    // step 4: assemble ReadyReport with post-publish observable state.
    // Note the read accessors are best-effort — if the underlying
    // store read fails we substitute defaults rather than fail the
    // whole join, since the caller's primary signal is `acked_by`.
    let members_learned = namespace_member_pubkeys(store, namespace_id)
        .map(|m| m.len())
        .unwrap_or(0);

    Ok(ReadyReport {
        namespace_id,
        final_head: [0u8; 32], // TODO(read accessor) — pull from DAG once ns-DAG read API is exposed
        applied_through: 0,    // TODO(read accessor) — same
        members_learned,
        acked_by: report.acked_by,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

/// Convenience aggregator: run the J6 fast path then the slow path.
///
/// `deadline` is split into a join slice and a ready slice with
/// `join = clamp(deadline / 3, 1s, deadline)`. The 1s floor prevents
/// `deadline / 3` from rounding to a near-zero value on small
/// deadlines; the cap stops the floor from EXCEEDING the caller's
/// total budget. Callers who want fine-grained control should call
/// `join_namespace` and `await_namespace_ready` directly.
pub async fn join_and_wait_ready(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    readiness_cache: &Arc<ReadinessCache>,
    readiness_notify: &Arc<ReadinessCacheNotify>,
    config: &ReadinessConfig,
    invitation: SignedGroupOpenInvitation,
    deadline: Duration,
) -> Result<ReadyReport, ReadyError> {
    let join_deadline = std::cmp::min(
        std::cmp::max(deadline / 3, Duration::from_secs(1)),
        deadline,
    );
    let started = join_namespace(
        store,
        node_client,
        readiness_cache,
        readiness_notify,
        config,
        invitation.clone(),
        join_deadline,
    )
    .await
    .map_err(|e| match e {
        JoinError::InvalidInvitation(msg) => ReadyError::InvalidInvitation(msg),
        other => ReadyError::JoinFailed(other.to_string()),
    })?;
    let ready_deadline = deadline.saturating_sub(join_deadline);
    await_namespace_ready(
        store,
        node_client,
        ack_router,
        invitation,
        started.namespace_id,
        ready_deadline,
    )
    .await
}

const ATTEMPT_DEADLINE: Duration = Duration::from_secs(10);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const INITIAL_BACKOFF: Duration = Duration::from_secs(3);

/// Retry [`join_namespace`] with exponential backoff + jitter.
///
/// Each attempt's deadline is clamped by the remaining total budget,
/// so a caller passing `max_total < ATTEMPT_DEADLINE` (e.g. 2s) does
/// NOT block on a single 10s attempt. The retry loop only re-enters
/// on `JoinError::NoReadyPeers` — invalid invitations and transport
/// errors are returned to the caller immediately because retrying
/// them is wasted work.
pub async fn join_namespace_with_retry(
    store: &Store,
    node_client: &NodeClient,
    readiness_cache: &Arc<ReadinessCache>,
    readiness_notify: &Arc<ReadinessCacheNotify>,
    config: &ReadinessConfig,
    invitation: SignedGroupOpenInvitation,
    max_total: Duration,
) -> Result<JoinStarted, JoinError> {
    let mut delay = INITIAL_BACKOFF;
    let start = Instant::now();
    loop {
        let remaining = max_total.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err(JoinError::NoReadyPeers {
                waited_ms: start.elapsed().as_millis() as u64,
            });
        }
        let attempt_deadline = std::cmp::min(ATTEMPT_DEADLINE, remaining);
        match join_namespace(
            store,
            node_client,
            readiness_cache,
            readiness_notify,
            config,
            invitation.clone(),
            attempt_deadline,
        )
        .await
        {
            Ok(started) => return Ok(started),
            Err(JoinError::NoReadyPeers { .. }) => {
                if start.elapsed() + delay > max_total {
                    return Err(JoinError::NoReadyPeers {
                        waited_ms: start.elapsed().as_millis() as u64,
                    });
                }
                let jitter_ms = {
                    let bound = delay.as_millis() as u64 / 4;
                    if bound == 0 {
                        0
                    } else {
                        rand::thread_rng().gen_range(0..bound)
                    }
                };
                tokio::time::sleep(delay + Duration::from_millis(jitter_ms)).await;
                delay = std::cmp::min(delay * 2, MAX_BACKOFF);
            }
            Err(other) => return Err(other),
        }
    }
}
