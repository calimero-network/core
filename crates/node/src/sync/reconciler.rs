//! Reconcile-after-divergence orchestration.
//!
//! Triggered when a signed namespace op arrives with a
//! [`DivergenceReport`] indicating per-context root-hash mismatch against
//! the signed canonical expected. For each divergent context, picks a
//! connected anchor peer (via the trusted-anchor set for the op's group
//! together with the verified [`SyncStateAccess::peer_identities`]
//! cache), initiates sync against that peer through the injected
//! [`ReconcileSyncDispatch`], and verifies the post-adoption root hash
//! against the signed expected.
//!
//! Extracted from `SyncManager` so the orchestration is unit-testable
//! without spinning up a `SyncSessionActor`: production wires
//! `SyncManager` as the dispatch (it implements [`ReconcileSyncDispatch`]
//! by forwarding to its own `initiate_sync`), tests pass a small mock.
//!
//! Backoff math + the per-context cooldown state operate on the
//! [`crate::state::NodeState::reconcile_attempts`] map and live as free
//! functions in this module so `SyncStateAccess`'s production impl in
//! `crate::state` and the reconciler itself share a single source of
//! truth. The fns are independently unit-testable.
use calimero_context::group_store::MembershipRepository;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use calimero_context_client::client::ContextClient;
use calimero_context_client::messages::DivergenceReport;
use calimero_context_config::types::ContextGroupId;
use calimero_node_primitives::sync::SyncProtocol;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use dashmap::DashMap;
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use rand::seq::SliceRandom;

use super::network::SyncNetwork;
use super::state_access::SyncStateAccess;
use crate::state::ReconcileAttempt;

/// Sync-initiation surface the reconciler depends on.
///
/// One method, mirroring `SyncManager::initiate_sync`'s signature.
/// Injected per-call rather than stored on the struct because
/// `SyncManager` owns the [`Reconciler`] field — storing a dispatch
/// reference back to `SyncManager` would create a self-referential
/// ownership cycle. Per-call injection sidesteps the cycle at the cost
/// of one extra method argument; the production wiring passes `&self`
/// (since `SyncManager: ReconcileSyncDispatch`), tests pass a mock.
///
/// `?Send` because `SyncManager::initiate_sync` is called from a single
/// async task (the namespace-event handler) and is not Send-safe
/// internally — `delta_store.rs` keeps a non-Send `Box<dyn Iterator>`
/// across an await in the merge path. The reconciler never spawns the
/// future cross-thread, so the relaxed bound is sound here. See the
/// existing `SyncNetwork` / `SyncStateAccess` traits which are
/// `Send + Sync` because their futures don't pull through that
/// non-Send iterator chain.
#[async_trait(?Send)]
pub(crate) trait ReconcileSyncDispatch {
    /// Open a sync session to `peer` for `context_id`. Returns the
    /// peer the session actually resolved against and the protocol
    /// the handshake selected.
    async fn initiate_sync(
        &self,
        context_id: ContextId,
        peer: PeerId,
    ) -> eyre::Result<(PeerId, SyncProtocol)>;
}

/// Reconcile-after-divergence orchestrator.
///
/// Holds Arcs of the state/network surfaces and a `ContextClient`
/// (cheap to clone). Constructed once by `SyncManager::new` and lives
/// for the manager's lifetime; clones of `SyncManager` clone the
/// reconciler too, sharing the underlying Arc surfaces.
#[derive(Clone)]
pub(crate) struct Reconciler {
    state_access: Arc<dyn SyncStateAccess>,
    sync_network: Arc<dyn SyncNetwork>,
    context_client: ContextClient,
}

impl Reconciler {
    pub(crate) fn new(
        state_access: Arc<dyn SyncStateAccess>,
        sync_network: Arc<dyn SyncNetwork>,
        context_client: ContextClient,
    ) -> Self {
        Self {
            state_access,
            sync_network,
            context_client,
        }
    }

    /// Handle a `DivergenceReport` from a signed namespace op.
    ///
    /// Empty `hash_differs` is the no-divergence case (debug log
    /// only) unless `group_hash_diverges` is set, in which case we
    /// surface the group-state-only divergence at warn (rare, but
    /// operator-visible). For each per-context mismatch, dispatch to
    /// [`Self::reconcile_one_divergent_context`].
    ///
    /// `only_in_expected` and `only_in_actual` entries are NOT
    /// reconciled here — those buckets reflect namespace-DAG drift (a
    /// registration the receiver hasn't seen yet, or one the signer
    /// hadn't seen). The cross-DAG membership check on subsequent
    /// state deltas catches that via `Unknown { needed }` → buffer;
    /// routing them through anchor sync would burn bandwidth on cases
    /// the existing catch-up path handles correctly.
    pub(crate) async fn reconcile_after_divergence<D: ReconcileSyncDispatch>(
        &self,
        dispatch: &D,
        report: DivergenceReport,
    ) {
        if report.hash_differs.is_empty() {
            // Distinguish "no divergence at all" (debug-level
            // bookkeeping) from "group-level divergence with no
            // per-context mismatch" (operator-visible: a member row
            // is missing or extra somewhere, but every context the
            // op touched still hashes the same). The latter is rare
            // enough that we want it surfaced, not buried at debug.
            // Per-context reconcile doesn't apply — there's no
            // signed canonical hash for the group-state alone to
            // pull state against — so we log and return. Subsequent
            // signed ops carry the corrected group-state hash and
            // the namespace-DAG buffer + cross-DAG check on later
            // state deltas closes the gap.
            if report.group_hash_diverges {
                tracing::warn!(
                    group_id = %hex::encode(report.group_id.to_bytes()),
                    op_kind = report.op_kind,
                    only_in_expected_count = report.only_in_expected.len(),
                    only_in_actual_count = report.only_in_actual.len(),
                    "reconcile-after-divergence: group-state hash diverges from signed expected, \
                     but no per-context hash mismatch is reconcilable here — convergence relies \
                     on the cross-DAG check against subsequent signed ops"
                );
            } else {
                tracing::debug!(
                    group_id = %hex::encode(report.group_id.to_bytes()),
                    op_kind = report.op_kind,
                    only_in_expected_count = report.only_in_expected.len(),
                    only_in_actual_count = report.only_in_actual.len(),
                    "reconcile-after-divergence: no per-context hash mismatches to reconcile; \
                     namespace-DAG drift (if any) is handled by the cross-DAG check on \
                     subsequent state deltas"
                );
            }
            return;
        }

        for (context_id, expected_root_hash) in &report.hash_differs {
            self.reconcile_one_divergent_context(
                dispatch,
                report.group_id,
                *context_id,
                *expected_root_hash,
                report.op_kind,
            )
            .await;
        }
    }

    /// Reconcile a single divergent context against a trusted anchor.
    ///
    /// Returns silently after logging — there is no error to bubble up
    /// to the caller because reconcile is best-effort: a future
    /// arrival of another signed op, or a sync interval tick, will
    /// re-attempt convergence. A hard error here would only inflate
    /// noise; the warn logs are the operator signal.
    ///
    /// Backoff: prior failed attempts for the same context impose an
    /// exponential cooldown (see [`reconcile_cooldown`]). Within that
    /// window, this is a no-op — the next signed op or sync tick will
    /// re-trigger once cooldown lapses. A successful post-adoption
    /// verify clears the backoff state immediately.
    ///
    /// **Convergence is not guaranteed in one shot**: `initiate_sync`
    /// negotiates the protocol via the standard handshake (typically
    /// `HashComparison` or `DeltaSync` between two initialized peers).
    /// Snapshot overwrite is gated by the `force=false` invariant in
    /// `fallback_to_snapshot_sync` and won't run on an initialized
    /// divergent node — that is by design, because snapshot adoption
    /// after the fact requires transactional staging the store layer
    /// doesn't yet provide. CRDT merge will sometimes converge two
    /// divergent states to the signed expected hash and sometimes
    /// won't (e.g. the partition-window case where the receiver holds
    /// a write the signer's expected hash excludes). When it doesn't,
    /// `verify_post_reconcile_root_hash` flags the mismatch and the
    /// backoff records a failure — operator-investigation territory
    /// until pre-adoption rejection + rollback lands.
    async fn reconcile_one_divergent_context<D: ReconcileSyncDispatch>(
        &self,
        dispatch: &D,
        group_id: ContextGroupId,
        context_id: ContextId,
        expected_root_hash: [u8; 32],
        op_kind: &'static str,
    ) {
        if let Some((remaining, failures)) =
            self.state_access.reconcile_remaining_cooldown(&context_id)
        {
            tracing::debug!(
                %context_id,
                op_kind,
                consecutive_failures = failures,
                cooldown_remaining_secs = remaining.as_secs(),
                "reconcile-after-divergence: skipping — prior attempts failed and the \
                 per-context cooldown is still active; will re-attempt after backoff lapses"
            );
            return;
        }

        // Look up anchors by `group_id` directly (carried in the
        // divergence report) rather than re-deriving the group from
        // `context_id`. A late-joiner can have a missing
        // context→group mapping locally even though the group's
        // trusted-anchor set is well-defined; the report already
        // names the group authoritatively so use it as the source of
        // truth.
        let anchors = self.anchor_identities_for_group(&group_id);
        if anchors.is_empty() {
            tracing::warn!(
                %context_id,
                group_id = %hex::encode(group_id.to_bytes()),
                op_kind,
                "reconcile-after-divergence: no trusted anchors defined for this group — \
                 falling back to operator path (no automatic recovery)"
            );
            return;
        }

        // Pick an anchor from the gossipsub mesh on the context's
        // topic. The mesh is a superset of "peers known to host this
        // context" — same source the regular sync path uses.
        //
        // Randomise the order before filtering so that, when there
        // are multiple connected anchors, we don't always pick the
        // one gossipsub happens to list first. Matters for two
        // reasons: (a) load distribution across honest anchors when
        // one is slow; (b) a compromised anchor that consistently
        // sorts first in libp2p's mesh order can't monopolise
        // reconcile syncs without contention. Post-adoption hash
        // verification against the signed expected still defends
        // against any anchor serving non-canonical state.
        let topic = TopicHash::from_raw(context_id);
        let mut mesh_peers = self.sync_network.subscribed_peers(topic).await;
        let mesh_peer_count = mesh_peers.len();
        mesh_peers.shuffle(&mut rand::thread_rng());
        // Walk mesh peers explicitly so cache-miss skips are visible
        // to operators. A peer with no `peer_identities` entry has not
        // yet been observed signing a verified message in this group;
        // it is invisible to the anchor predicate even if it would be
        // an anchor in practice. Counting and logging those skips
        // distinguishes "no anchors reachable" from "anchors reachable
        // but cache hasn't warmed yet" in the no-anchor warn below.
        let mut peers_missing_cache_entry: usize = 0;
        let mut peers_known_not_anchor: usize = 0;
        let anchor_peer =
            mesh_peers
                .iter()
                .copied()
                .find(|peer| match self.state_access.peer_identities(peer) {
                    Some(ids) => {
                        if ids.iter().any(|id| anchors.contains(id)) {
                            true
                        } else {
                            peers_known_not_anchor += 1;
                            false
                        }
                    }
                    None => {
                        peers_missing_cache_entry += 1;
                        tracing::debug!(
                            %context_id,
                            %peer,
                            op_kind,
                            "reconcile-after-divergence: mesh peer skipped — no peer_identities \
                             cache entry yet (peer has not been observed signing a verified \
                             message); cache warms as the peer's signed traffic is processed"
                        );
                        false
                    }
                });
        let Some(anchor_peer) = anchor_peer else {
            tracing::warn!(
                %context_id,
                op_kind,
                anchor_count = anchors.len(),
                connected_mesh_peers = mesh_peer_count,
                peers_missing_cache_entry,
                peers_known_not_anchor,
                "reconcile-after-divergence: no connected mesh peer matches the anchor set — \
                 falling back to operator path; reconcile will re-attempt on the next signed \
                 op or sync tick"
            );
            return;
        };

        tracing::info!(
            %context_id,
            %anchor_peer,
            op_kind,
            expected_root_hash = %hex::encode(expected_root_hash),
            "reconcile-after-divergence: pulling canonical state from trusted anchor"
        );

        match dispatch.initiate_sync(context_id, anchor_peer).await {
            Ok((peer_used, protocol)) => {
                tracing::info!(
                    %context_id,
                    %peer_used,
                    ?protocol,
                    "reconcile-after-divergence: anchor sync completed; verifying post-adoption hash"
                );
                // Use `peer_used` (the peer the sync actually
                // resolved against) for verify-time logs rather than
                // the originally-picked `anchor_peer`. The two
                // normally agree, but `initiate_sync` is the
                // authoritative source.
                let converged = self.verify_post_reconcile_root_hash(
                    context_id,
                    expected_root_hash,
                    peer_used,
                    op_kind,
                );
                if converged {
                    self.state_access.record_reconcile_success(&context_id);
                } else {
                    let failures = self.state_access.record_reconcile_failure(context_id);
                    tracing::warn!(
                        %context_id,
                        op_kind,
                        consecutive_failures = failures,
                        next_cooldown_secs = reconcile_cooldown(failures).as_secs(),
                        "reconcile-after-divergence: recorded failure; subsequent reconcile \
                         attempts for this context are gated by the backoff window"
                    );
                }
            }
            Err(err) => {
                let failures = self.state_access.record_reconcile_failure(context_id);
                tracing::warn!(
                    %context_id,
                    %anchor_peer,
                    op_kind,
                    %err,
                    consecutive_failures = failures,
                    next_cooldown_secs = reconcile_cooldown(failures).as_secs(),
                    "reconcile-after-divergence: anchor sync failed; reconcile will re-attempt \
                     after the backoff window lapses"
                );
            }
        }
    }

    /// Compare the local context's `root_hash` against the signed
    /// `expected_root_hash` from the triggering op. On match, log at
    /// info level — the reconcile succeeded. On mismatch, log loudly
    /// at warn: the anchor served state that does not match the
    /// canonical expected, OR the local apply diverged again after
    /// sync. Either is operator-investigation territory and a
    /// follow-up will replace this post-adoption check with
    /// pre-adoption rejection + rollback once the store layer has
    /// transactional staging.
    fn verify_post_reconcile_root_hash(
        &self,
        context_id: ContextId,
        expected_root_hash: [u8; 32],
        anchor_peer: PeerId,
        op_kind: &'static str,
    ) -> bool {
        let Ok(Some(context)) = self.context_client.get_context(&context_id) else {
            tracing::warn!(
                %context_id,
                %anchor_peer,
                op_kind,
                "reconcile-after-divergence: context not found locally after anchor sync — \
                 cannot verify root hash"
            );
            return false;
        };

        let actual_root_hash: [u8; 32] = *AsRef::<[u8; 32]>::as_ref(&context.root_hash);
        if actual_root_hash == expected_root_hash {
            tracing::info!(
                %context_id,
                %anchor_peer,
                op_kind,
                root_hash = %hex::encode(actual_root_hash),
                "reconcile-after-divergence: post-adoption hash matches signed expected — converged"
            );
            true
        } else {
            // ERROR, not WARN: this is the authoritative "recovery ran and
            // FAILED" signal. We pulled canonical state from a *trusted anchor*
            // and the root STILL doesn't match the *signed* expected hash — a
            // confirmed, non-transient split-brain (unlike the heartbeat's
            // unauthenticated peer-hash observation, which is gated to WARN
            // until it persists). When this fires it is genuinely
            // operator-investigation territory and should page / fail CI.
            tracing::error!(
                %context_id,
                %anchor_peer,
                op_kind,
                expected_root_hash = %hex::encode(expected_root_hash),
                actual_root_hash = %hex::encode(actual_root_hash),
                "reconcile-after-divergence: post-adoption hash does NOT match signed expected — \
                 either the anchor served non-canonical state or local apply diverged again; \
                 recovery from a trusted anchor failed, confirmed split-brain"
            );
            false
        }
    }

    /// Look up the trusted-anchor identity set for a group directly.
    /// Preferred over a context-keyed lookup when the caller already
    /// knows `group_id` — late-joiner nodes can have a missing
    /// context→group mapping, which makes the context-keyed lookup
    /// return an empty set even though the group's anchors are
    /// well-defined on the local node.
    fn anchor_identities_for_group(&self, group_id: &ContextGroupId) -> BTreeSet<PublicKey> {
        let store = self.context_client.datastore_handle().into_inner();
        MembershipRepository::new(&store)
            .trusted_anchors(group_id)
            .unwrap_or_default()
    }
}

// =========================================================================
// Backoff math + cooldown-state helpers
// =========================================================================

/// Exponential cooldown for the reconcile-after-divergence backoff,
/// capped at 30 min. `consecutive_failures == 0` is illegal (the
/// caller only invokes this when at least one failure has been
/// recorded); we treat it the same as `1` to avoid an arithmetic
/// surprise. Schedule:
///
/// - 1 failure → 30s
/// - 2 failures → 60s
/// - 3 failures → 2m
/// - 4 failures → 4m
/// - 5 failures → 8m
/// - 6 failures → 16m
/// - 7+ failures → 30m (cap)
///
/// Free function so backoff math can be unit-tested independently.
pub(crate) fn reconcile_cooldown(consecutive_failures: u32) -> Duration {
    const BASE_SECS: u64 = 30;
    const MAX: Duration = Duration::from_secs(30 * 60);
    let exp = consecutive_failures.saturating_sub(1).min(8);
    let secs = BASE_SECS.saturating_mul(1u64 << u64::from(exp));
    Duration::from_secs(secs).min(MAX)
}

/// If `context_id` has a recorded prior failure that is still within
/// its cooldown window, return `Some((remaining_cooldown,
/// consecutive_failures))`. Otherwise — no entry, or the cooldown has
/// elapsed — return `None`.
pub(crate) fn reconcile_remaining_cooldown(
    attempts: &DashMap<ContextId, ReconcileAttempt>,
    context_id: &ContextId,
) -> Option<(Duration, u32)> {
    let entry = attempts.get(context_id)?;
    let cooldown = reconcile_cooldown(entry.consecutive_failures);
    let elapsed = entry.last_attempt_at.elapsed();
    let remaining = cooldown.checked_sub(elapsed)?;
    if remaining.is_zero() {
        None
    } else {
        Some((remaining, entry.consecutive_failures))
    }
}

/// Record a reconcile failure for `context_id`: bump
/// `consecutive_failures` and stamp `last_attempt_at = now`. Returns
/// the new failure count so the caller can log the next cooldown
/// directly.
pub(crate) fn record_reconcile_failure(
    attempts: &DashMap<ContextId, ReconcileAttempt>,
    context_id: ContextId,
) -> u32 {
    let mut entry = attempts
        .entry(context_id)
        .or_insert_with(|| ReconcileAttempt {
            last_attempt_at: Instant::now(),
            consecutive_failures: 0,
        });
    entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
    entry.last_attempt_at = Instant::now();
    entry.consecutive_failures
}

/// Clear backoff state for `context_id` after a successful reconcile.
/// Subsequent divergences are treated as fresh — no inherited cooldown.
pub(crate) fn record_reconcile_success(
    attempts: &DashMap<ContextId, ReconcileAttempt>,
    context_id: &ContextId,
) {
    let _ = attempts.remove(context_id);
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    // Method-level orchestration tests (e.g. cooldown short-circuit,
    // no-anchor warn paths, successful-sync hash-verify, failed-sync
    // backoff recording) need a `ContextClient` that the `MockSyncNetwork`
    // / `MockSyncStateAccess` pair doesn't currently provide a fixture
    // for. Those tests will land in a follow-up alongside a lightweight
    // ContextClient test-fixture helper. This module covers the backoff
    // math + cooldown-state helpers, which are the higher-leverage
    // surface and the only part `state.rs`'s `SyncStateAccess` impl
    // depends on at runtime.
    use super::*;

    fn ctx(byte: u8) -> ContextId {
        ContextId::from([byte; 32])
    }

    #[test]
    fn reconcile_cooldown_schedule_doubles_then_caps() {
        assert_eq!(reconcile_cooldown(1), Duration::from_secs(30));
        assert_eq!(reconcile_cooldown(2), Duration::from_secs(60));
        assert_eq!(reconcile_cooldown(3), Duration::from_secs(120));
        assert_eq!(reconcile_cooldown(4), Duration::from_secs(240));
        assert_eq!(reconcile_cooldown(5), Duration::from_secs(480));
        assert_eq!(reconcile_cooldown(6), Duration::from_secs(960));
        assert_eq!(reconcile_cooldown(7), Duration::from_secs(30 * 60));
        // Cap holds for arbitrarily large counters.
        assert_eq!(reconcile_cooldown(50), Duration::from_secs(30 * 60));
        assert_eq!(reconcile_cooldown(u32::MAX), Duration::from_secs(30 * 60));
    }

    #[test]
    fn reconcile_cooldown_zero_failures_treated_as_one() {
        // The function is only meant to be called when at least one
        // failure has been recorded; we still want a defined value at
        // 0 rather than a panic or underflow.
        assert_eq!(reconcile_cooldown(0), Duration::from_secs(30));
    }

    #[test]
    fn record_reconcile_failure_increments_counter_and_stamps_time() {
        let attempts: DashMap<ContextId, ReconcileAttempt> = DashMap::new();
        let context = ctx(1);

        assert_eq!(record_reconcile_failure(&attempts, context), 1);
        assert_eq!(record_reconcile_failure(&attempts, context), 2);
        assert_eq!(record_reconcile_failure(&attempts, context), 3);

        let entry = attempts.get(&context).expect("entry was inserted");
        assert_eq!(entry.consecutive_failures, 3);
        // Stamp should be very recent (within the last few seconds).
        assert!(entry.last_attempt_at.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn record_reconcile_success_clears_entry() {
        let attempts: DashMap<ContextId, ReconcileAttempt> = DashMap::new();
        let context = ctx(1);

        let _ = record_reconcile_failure(&attempts, context);
        let _ = record_reconcile_failure(&attempts, context);
        assert!(attempts.contains_key(&context));

        record_reconcile_success(&attempts, &context);
        assert!(
            !attempts.contains_key(&context),
            "success should clear all backoff state for the context"
        );
    }

    #[test]
    fn reconcile_remaining_cooldown_none_when_no_entry() {
        let attempts: DashMap<ContextId, ReconcileAttempt> = DashMap::new();
        assert!(reconcile_remaining_cooldown(&attempts, &ctx(1)).is_none());
    }

    #[test]
    fn reconcile_remaining_cooldown_some_after_recent_failure() {
        let attempts: DashMap<ContextId, ReconcileAttempt> = DashMap::new();
        let context = ctx(1);
        let _ = record_reconcile_failure(&attempts, context);

        let (remaining, failures) =
            reconcile_remaining_cooldown(&attempts, &context).expect("within cooldown");
        assert_eq!(failures, 1);
        // The first cooldown is 30 s; the test runs in <1 s.
        assert!(remaining > Duration::from_secs(25));
        assert!(remaining <= Duration::from_secs(30));
    }

    #[test]
    fn reconcile_remaining_cooldown_none_after_cooldown_lapsed() {
        let attempts: DashMap<ContextId, ReconcileAttempt> = DashMap::new();
        let context = ctx(1);
        // Synthesize an entry whose timestamp is far enough in the
        // past that even the maximum cooldown has lapsed.
        let _replaced = attempts.insert(
            context,
            ReconcileAttempt {
                last_attempt_at: Instant::now() - Duration::from_secs(60 * 60),
                consecutive_failures: 7,
            },
        );
        assert!(reconcile_remaining_cooldown(&attempts, &context).is_none());
    }
}
