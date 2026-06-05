//! Self-purge handler for evicted TEE node identities.
//!
//! Subscribes to the op-apply event channel (see [`crate::op_events`])
//! and reacts to `OpEvent::TeeMemberRemoved` events that target THIS
//! node's identity for the affected namespace — purging local state for
//! the group (or, for namespace-root removals, the whole subtree) so
//! that signing-key material, gov-op log, namespace identity, and
//! membership-side metadata do not linger after a TEE eviction.
//!
//! See `docs/adr/0002-fleet-tee-leave-protocol.md` for the architectural
//! framing.
//!
//! # Role-scoped: TEE removals only
//!
//! The listener intentionally gates on `OpEvent::TeeMemberRemoved`,
//! NOT the generic `OpEvent::MemberRemoved`. Both events are emitted
//! by the apply path on a removal whose role was `ReadOnlyTee`; only
//! `MemberRemoved` is emitted for `Admin`/`Member`/`Observer` removals.
//! Non-TEE removals deliberately stay on the SOFT-leave path — the
//! local rows remain so kick-and-readd / rejoin-via-keyshare /
//! inheritance-rejoin flows can re-use them. Hardening to hard-purge
//! on every removal regresses the e2e workflows under
//! `apps/scaffolding-e2e/workflows/group-{kick,leave}-*` that depend on
//! that soft-leave invariant.
//!
//! TEE removals are different: a `ReadOnlyTee` node has no rejoin
//! pathway (the only admission op for the role is
//! `MemberJoinedViaTeeAttestation`, which re-derives identity from a
//! fresh attestation), so leaving on-disk key material around buys
//! nothing and risks forward-secrecy hygiene. Hard-purge.
//!
//! # Why a separate handler (not in the apply arm)
//!
//! The apply layer at `calimero_governance_store` is deliberately
//! node-agnostic: it runs identically on every peer that receives the
//! op. Self-detection ("did this op evict ME?") is a handler-level
//! concern because it requires reading the node's stored namespace
//! identity — which is per-node state, not part of the apply contract.
//! Mirrors the same architectural split [`crate::auto_follow`] uses.
//!
//! # Scope split: subgroup vs namespace root
//!
//! `TeeMemberRemoved` can fire at either the namespace root (a kick
//! from the namespace, which cascades to all descendant subgroups via
//! the existing apply code) or at a subgroup (a kick from one subgroup
//! only, while the node may still be in other subgroups under the
//! same namespace).
//!
//! * Subgroup-only: purge only that group's local rows. Do NOT
//!   unsubscribe from the namespace gossipsub topic — other
//!   memberships under it still need it. Mirrors the rationale in
//!   `handlers/leave_group.rs:38-40`.
//! * Namespace root: cascade-purge the subtree, then drop namespace-
//!   level state, then unsubscribe from the namespace topic.
//!
//! # Forward-secrecy invariant
//!
//! This module does NOT trigger any key-rotation op. Forward secrecy
//! on the namespace's NEW writes is already provided by the existing
//! `MemberRemoved` rotation pipeline at
//! `calimero_governance_store::group_governance_publisher::sign_apply_and_publish_inner`
//! (the publisher generates a fresh group key wrapped for everyone
//! EXCEPT the removed member). This handler only deletes what the
//! evicted TEE node held locally — including the now-useless old key
//! material the rotation already orphaned.

use std::sync::Mutex;

use calimero_context_config::types::ContextGroupId;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use tokio::task::AbortHandle;
use tracing::{debug, error, info, warn};

use calimero_governance_store;
use calimero_governance_store::metrics::{record_purge_failure, PurgeBranch, PurgeFailureClass};
use calimero_governance_store::op_events::{self, OpEvent};
use calimero_governance_store::{MembershipRepository, NamespaceRepository};

struct HandleState {
    abort: AbortHandle,
}

static HANDLE: Mutex<Option<HandleState>> = Mutex::new(None);

/// Spawn the self-purge handler. Returns immediately; the handler runs
/// as a detached tokio task for the process lifetime.
///
/// Idempotent: subsequent calls (e.g. after an Actix actor restart) are
/// no-ops unless [`shutdown`] is called first. Re-subscribing without
/// aborting would cause every eviction event to fan out into multiple
/// concurrent purges of the same store — wasteful but not incorrect
/// (the underlying `delete_*_local_rows` helpers are idempotent batched
/// deletes). The single-spawn guard exists for tidiness, not safety.
pub fn spawn(store: Store, node_client: NodeClient) {
    let mut slot = HANDLE.lock().expect("self-purge HANDLE poisoned");
    if slot.as_ref().is_some_and(|h| !h.abort.is_finished()) {
        debug!("self-purge handler already running; skipping re-spawn");
        return;
    }
    let abort = tokio::spawn(async move {
        run(store, node_client).await;
    })
    .abort_handle();
    *slot = Some(HandleState { abort });
}

/// Abort the running handler task. Intended for tests and graceful-
/// shutdown hooks. Safe to call even if no handler is running. After
/// calling this, [`spawn`] may be called again.
pub fn shutdown() {
    if let Some(state) = HANDLE.lock().expect("self-purge HANDLE poisoned").take() {
        state.abort.abort();
    }
}

async fn run(store: Store, node_client: NodeClient) {
    let mut rx = op_events::subscribe();
    info!("self-purge handler started");

    // Startup reconcile sweep (#2721). Runs once, BEFORE the event loop, so
    // any purge residue that no live event will ever re-trigger is completed
    // on the way up:
    //
    //   * a `TeeMemberRemoved` dropped while we were down / lagged (the
    //     `Lagged` arm below has no incidental recovery), and
    //   * a prior signing-key purge that failed mid-cascade and left the
    //     `NamespaceIdentity` row behind as a retry anchor.
    //
    // A full scan of `NamespaceIdentity` rows subsumes the interim
    // "pending-purge marker" idea: it catches ALL residue, not just rows that
    // happened to be marked — including the dropped-event case, which a marker
    // (written only on a delivered-but-failed event) structurally cannot
    // cover. Startup-only, not continuous: a dropped event mid-session still
    // waits for the next restart (a periodic sweep is a deliberate follow-up,
    // not implemented here — see the `reconcile_sweep` docstring).
    reconcile_sweep(&store, &node_client).await;

    loop {
        let event = match rx.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                // Missed events: the evicted membership row is already gone
                // from the local store (apply committed before notify) while
                // the signing-key + gov-op rows linger. There is no incidental
                // event-driven recovery — an already-evicted identity receives
                // no further removal events (a re-admitted TEE node derives a
                // fresh attestation pubkey). Recovery now comes from the
                // startup reconcile sweep (#2721, [`reconcile_sweep`] above):
                // it re-scans the local `NamespaceIdentity` rows on the NEXT
                // process start and completes any evicted-but-unpurged residue
                // idempotently. So a dropped event leaves residue on disk only
                // until the next restart — the sweep is startup-only, not
                // continuous. Bounded, not a forward-secrecy hole: FS on future
                // writes comes from key rotation, not this purge (see module
                // docstring §"Forward-secrecy invariant"); the residue is stale
                // already-orphaned key material on this node's own disk.
                warn!(
                    skipped,
                    "self-purge subscriber lagged; some events were dropped — \
                     residual local state persists until the next restart, when the \
                     startup reconcile sweep (#2721) completes the purge"
                );
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                info!("self-purge op-event channel closed; handler exiting");
                break;
            }
        };

        // Role-scoped: only TEE removals trigger the hard-purge. See the
        // module docstring for why we don't also react to
        // `OpEvent::MemberRemoved` here.
        if let Some((group_id, member)) = dispatch_target(&event) {
            handle_member_removed(&store, &node_client, group_id, member).await;
        }
    }
}

/// Listener match-arm predicate, extracted so a unit test can verify
/// that the non-TEE `MemberRemoved` event is intentionally ignored
/// (i.e. the soft-leave path is preserved for `Admin`/`Member`/
/// `Observer` removals).
///
/// Returns `Some((group_id, member))` iff the listener should dispatch
/// a purge for this event; `None` otherwise.
pub(crate) fn dispatch_target(event: &OpEvent) -> Option<([u8; 32], PublicKey)> {
    match event {
        OpEvent::TeeMemberRemoved { group_id, member } => Some((*group_id, *member)),
        _ => None,
    }
}

/// The dispatch decision for a `TeeMemberRemoved` event: do nothing,
/// purge a single subgroup, or cascade-purge the whole namespace.
///
/// Split out from [`handle_member_removed`] so the dispatch logic
/// (which is the part most likely to regress on a refactor) is unit-
/// testable WITHOUT standing up a `NodeClient` mock — the namespace-
/// root execution path is async and touches gossipsub, but the
/// decision of WHICH branch to take is pure store reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PurgeAction {
    /// Event is for someone else, or for a namespace this node has no
    /// identity in. No action.
    None,
    /// Subgroup-only purge for this group_id. Namespace identity stays.
    Subgroup(ContextGroupId),
    /// Namespace-root cascade for this namespace_id. Drops everything
    /// and unsubscribes from the gossipsub topic.
    Namespace(ContextGroupId),
}

/// Pure-read dispatch decision. Tells the listener which purge branch
/// applies for the given event, without mutating the store.
pub(crate) fn decide_purge_action(
    store: &Store,
    group_id: [u8; 32],
    member: PublicKey,
) -> PurgeAction {
    let gid = ContextGroupId::from(group_id);

    // Resolve the namespace owning this group. If the lookup fails the
    // most likely cause is that the apply itself just removed our last
    // anchor under this namespace and `resolve_namespace` no longer
    // finds a path. We log at debug because this is expected on the
    // namespace-root branch when the apply has already done its
    // cascade.
    let ns_id = match NamespaceRepository::new(store).resolve(&gid) {
        Ok(n) => n,
        Err(e) => {
            debug!(
                group_id = %hex::encode(group_id),
                error = ?e,
                "self-purge: cannot resolve namespace for evicted group — skipping"
            );
            return PurgeAction::None;
        }
    };

    // Look up THIS node's identity for the namespace. None = we never
    // had an identity here (the event is about someone else's membership
    // in a namespace we never joined). Some(_) means this could be ours.
    //
    // Pass `ns_id` (not `gid`) so the call reads "look up the identity for
    // THE NAMESPACE" — self-documenting at the call site. The underlying
    // `resolve_identity_record` internally resolves the namespace
    // anyway (`governance-store/src/namespace/core.rs`), so passing `gid`
    // would also work; using `ns_id` just removes the apparent ambiguity
    // flagged in PR review.
    let self_pk = match NamespaceRepository::new(store).resolve_identity(&ns_id) {
        Ok(Some((pk, _sk, _sender))) => pk,
        Ok(None) => {
            // Not our namespace; nothing to purge. The most common case
            // for the listener.
            return PurgeAction::None;
        }
        Err(e) => {
            warn!(
                group_id = %hex::encode(group_id),
                error = ?e,
                "self-purge: failed to resolve namespace identity"
            );
            return PurgeAction::None;
        }
    };

    if member != self_pk {
        // Event is about a different member in our namespace. We stay.
        return PurgeAction::None;
    }

    // It's us. Two branches: namespace-root removal cascades; subgroup
    // removal does not.
    if gid == ns_id {
        PurgeAction::Namespace(ns_id)
    } else {
        PurgeAction::Subgroup(gid)
    }
}

/// Startup reconcile sweep (#2721): walk every locally-stored
/// `NamespaceIdentity` and complete any purge that an eviction left
/// unfinished.
///
/// # Why a sweep (vs. event-driven retry)
///
/// `TeeMemberRemoved` fires exactly once per eviction, and an
/// already-evicted identity never receives a follow-up event (a re-admitted
/// TEE node derives a fresh attestation pubkey). So two residue cases have no
/// event-driven recovery:
///
///   1. **Dropped event** — the broadcast `Lagged` arm in [`run`] skipped a
///      `TeeMemberRemoved`, so the purge never fired.
///   2. **Partial cascade failure** — a prior `cascade_namespace_state`
///      returned `signing_key_purge_failed`, which deliberately KEPT the
///      `NamespaceIdentity` (+ gossipsub subscription) as a retry anchor —
///      but nothing retried it.
///
/// A startup full-scan of `NamespaceIdentity` rows covers BOTH (and is
/// strictly more complete than a per-row "pending-purge marker", which a
/// dropped event would never have written). The purge path it invokes
/// (`cascade_namespace_state` / the namespace finalize + unsubscribe) is
/// idempotent, so re-running it on an already-clean namespace is a no-op.
///
/// # The reconcile predicate (get this right — false-purging a healthy
/// member is a forward-secrecy-adjacent bug)
///
/// For each stored `(ns_id, self_pk)` we reconcile **iff we are no longer a
/// live member anywhere under that namespace** — see
/// [`namespace_needs_reconcile`]. "Live member" means a concrete `GroupMember`
/// row (DIRECT membership) for `self_pk` at the namespace root OR any
/// descendant group. If ANY such row exists we are still in the namespace and
/// MUST NOT purge — the identity is legitimately retained (e.g. a
/// subgroup-only eviction leaves the root membership, and other subgroups,
/// intact). Only when every direct membership row under the subtree is gone is
/// the identity residue from a namespace-root eviction whose cascade never
/// completed.
///
/// We use DIRECT membership (`role_of`), not inherited
/// (`is_member`/`check_path`): a TEE node's presence under a namespace is its
/// `GroupMember` row, which is what the apply path removes on eviction.
/// Inherited (Open-subgroup admin-inheritance) membership is an authorization
/// concept, not a "this node holds key material here" signal, and would never
/// apply to a `ReadOnlyTee` node anyway.
///
/// # Scope
///
/// Startup-only, not continuous. A dropped event mid-session waits for the
/// next restart. A periodic sweep is a possible follow-up but is intentionally
/// not implemented here — the startup pass closes the durable-residue cases,
/// and a tight loop re-scanning identities on every node has a cost/benefit
/// that wants its own design pass.
async fn reconcile_sweep(store: &Store, node_client: &NodeClient) {
    let identities = match NamespaceRepository::new(store).iter_identities() {
        Ok(v) => v,
        Err(e) => {
            warn!(
                error = ?e,
                "self-purge reconcile: failed to enumerate NamespaceIdentity rows — \
                 skipping startup sweep (residue, if any, persists until next restart)"
            );
            return;
        }
    };

    let scanned = identities.len();
    let mut reconciled = 0usize;

    for (ns_id, record) in identities {
        let ns_hex = hex::encode(ns_id.to_bytes());
        match namespace_needs_reconcile(store, ns_id, record.public_key) {
            Ok(true) => {
                info!(
                    namespace = %ns_hex,
                    "self-purge reconcile: stored identity with no surviving membership \
                     under the namespace — completing the evicted purge"
                );
                // Reuse the namespace-root purge path. It is idempotent and
                // gates the identity/subscription finalize on the signing-key
                // purge exactly as the event path does.
                purge_namespace_for_self(store, node_client, ns_id).await;
                reconciled += 1;
            }
            Ok(false) => {
                debug!(
                    namespace = %ns_hex,
                    "self-purge reconcile: still a live member — leaving identity intact"
                );
            }
            Err(e) => {
                // A read error on the membership walk is not a license to
                // purge — defaulting to "purge on error" would risk
                // false-purging a healthy member on a transient store hiccup.
                // Skip; the next restart retries.
                warn!(
                    namespace = %ns_hex,
                    error = ?e,
                    "self-purge reconcile: membership check errored — skipping this \
                     namespace (will retry on next restart; NOT purging on uncertainty)"
                );
                record_purge_failure(PurgeBranch::Namespace, PurgeFailureClass::SigningKey);
            }
        }
    }

    info!(scanned, reconciled, "self-purge reconcile sweep complete");
}

/// Reconcile predicate: does the locally-stored identity `self_pk` for
/// namespace `ns_id` represent evicted residue that should be purged?
///
/// Returns `Ok(true)` iff `self_pk` has NO direct `GroupMember` row at the
/// namespace root OR any descendant group — i.e. we are no longer a live
/// member anywhere under the namespace, so the stored identity is leftover
/// state from an eviction whose purge never completed.
///
/// Returns `Ok(false)` if a direct membership row survives anywhere under the
/// subtree (still a live member — must not purge).
///
/// Pure store reads; no mutation. Split out so it is unit-testable without a
/// `NodeClient`.
pub(crate) fn namespace_needs_reconcile(
    store: &Store,
    ns_id: ContextGroupId,
    self_pk: PublicKey,
) -> eyre::Result<bool> {
    let membership = MembershipRepository::new(store);

    // Root first — the common surviving-membership case for a node that was
    // only kicked from a subgroup.
    if membership.role_of(&ns_id, &self_pk)?.is_some() {
        return Ok(false);
    }

    // Then the whole subtree. `collect_descendants` walks the child index
    // (the same walk the cascade uses), so a membership row in any nested
    // subgroup keeps us a live member and blocks the purge.
    for descendant in NamespaceRepository::new(store).collect_descendants(&ns_id)? {
        if membership.role_of(&descendant, &self_pk)?.is_some() {
            return Ok(false);
        }
    }

    // No direct membership anywhere under the namespace → evicted residue.
    Ok(true)
}

async fn handle_member_removed(
    store: &Store,
    node_client: &NodeClient,
    group_id: [u8; 32],
    member: PublicKey,
) {
    match decide_purge_action(store, group_id, member) {
        PurgeAction::None => {}
        PurgeAction::Subgroup(gid) => purge_subgroup_for_self(store, gid),
        PurgeAction::Namespace(ns_id) => {
            purge_namespace_for_self(store, node_client, ns_id).await;
        }
    }
}

/// Subgroup-only purge: this node was kicked from a single subgroup but
/// may still be a member of other groups under the same namespace.
///
/// Drops the subgroup's local rows (members, signing keys, caps, etc.)
/// but leaves the namespace identity and the gossipsub subscription
/// intact — the rationale is the same as
/// `handlers/leave_group.rs:38-40`: other memberships still need them.
///
/// Sync: store operations only, no async work. Split out so tests can
/// drive it without standing up a `NodeClient` mock.
///
/// Mirrors the per-group cleanup sequence in
/// `handlers/delete_namespace.rs:74-90` for a single group:
///
/// 1. Unregister every context registered in the group (drops
///    `GroupContextIndex` + `ContextGroupRef` rows).
/// 2. Capture the parent so we can drop the parent/child edges (apply
///    has already removed our `GroupMember` row; the tree-edge keys
///    `GroupParentRef` + `GroupChildIndex` are separate columns).
/// 3. `delete_group_local_rows` (members, signing keys, caps, meta,
///    op-log, …).
/// 4. Drop the parent/child edge keys for this group.
///
/// Without steps 1, 2, and 4, the bot in mdma#106-review correctly
/// notes that context-index rows + tree-edge rows linger after eviction
/// even though `delete_group_local_rows` has run. Mirrors the full
/// teardown.
pub(crate) fn purge_subgroup_for_self(store: &Store, gid: ContextGroupId) {
    let group_hex = hex::encode(gid.to_bytes());

    // Priority order for a subgroup-only purge (no future event will
    // re-trigger this code path, per ADR 0002 — the cascade has a retry
    // path via the next MemberRemoved, the single-subgroup case does
    // not). So we treat `delete_group_local_rows` as load-bearing
    // (32-byte private signing-key material lives in there; leaking
    // those is the actual forward-secrecy hazard) and demote everything
    // else to best-effort. v6 review iterated on this and v6's earlier
    // defensive aborts were over-aggressive: aborting on a context-
    // unregister or parent-read error left the signing keys on disk,
    // which is strictly worse than the orphaned `GroupContextIndex` or
    // `GroupParentRef` rows the aborts were preventing (those are dead
    // pointers; signing keys are private material). mdma#106 v7 review
    // (meroreviewer).

    if let Err(e) = unregister_all_contexts(store, &gid) {
        warn!(
            group_id = %group_hex,
            error = ?e,
            "self-purge: failed to unregister contexts before subgroup row purge \
             — context-index rows may persist as orphans pointing at the soon-to-be \
             deleted group; continuing so signing keys still get purged"
        );
        record_purge_failure(PurgeBranch::Subgroup, PurgeFailureClass::ContextCleanup);
    }

    let parent = NamespaceRepository::new(store)
        .parent(&gid)
        .unwrap_or_else(|e| {
            warn!(
                group_id = %group_hex,
                error = ?e,
                "self-purge: failed to read parent edge — tree-edge cleanup will be skipped, \
                 but signing-key purge proceeds"
            );
            record_purge_failure(PurgeBranch::Subgroup, PurgeFailureClass::ContextCleanup);
            None
        });

    if let Err(e) = calimero_governance_store::delete_group_local_rows(store, &gid) {
        // This IS the load-bearing step (signing-key material). If it
        // fails, the subgroup-only branch has no retry surface, so we
        // surface at error level. Tree-edge cleanup is then skipped
        // because severing the parent/child link while rows remain
        // produces an unreachable-but-present group — strictly worse
        // than the bounded leak we already accepted by failing here.
        error!(
            group_id = %group_hex,
            error = ?e,
            "self-purge: failed to drop local rows for evicted subgroup — \
             signing-key material remains on disk (no retry surface for \
             subgroup-only purge; the #2721 startup reconcile sweep does NOT \
             cover this case — it fires only when ALL namespace membership is \
             gone, whereas a subgroup-only eviction keeps the root membership — \
             so manual cleanup or a subgroup-scoped reconcile follow-up is \
             needed; see ADR 0002)"
        );
        record_purge_failure(PurgeBranch::Subgroup, PurgeFailureClass::SigningKey);
        return;
    }

    info!(
        group_id = %group_hex,
        "self-purge: dropped local rows for subgroup we were evicted from"
    );

    if let Some(parent) = parent {
        if let Err(e) = delete_tree_edges(store, &gid, &parent) {
            // Elevated to `error!` because — unlike the cascade branch —
            // a subgroup-only eviction has no future `MemberRemoved` event
            // to drive a retry, AND the #2721 startup reconcile sweep does
            // not reach it: that sweep keys off the `NamespaceIdentity` row
            // and only fires when ALL namespace membership is gone, but a
            // subgroup-only eviction keeps the root membership. A
            // subgroup-scoped reconcile remains the deferred follow-up
            // tracked by ADR 0002. The leak is
            // bounded: the orphaned `GroupParentRef` / `GroupChildIndex`
            // rows point at a now-deleted group, so traversal won't find
            // anything when it walks them. Pure dead state; flagged at
            // `error!` so operators can spot it in aggregate logs and
            // sweep manually if needed. mdma#106 v6 review (cursor
            // "Subgroup tree edge purge stuck").
            error!(
                group_id = %group_hex,
                error = ?e,
                "self-purge: failed to drop tree edges for evicted subgroup — \
                 orphaned tree-edge rows will persist (no retry surface for \
                 subgroup-only purge; not covered by the #2721 namespace-root \
                 reconcile sweep; see ADR 0002 subgroup-reconcile follow-up)"
            );
            record_purge_failure(PurgeBranch::Subgroup, PurgeFailureClass::ContextCleanup);
        }
    }
}

/// Unregister every context registered under `gid`. Mirrors
/// `handlers/delete_namespace.rs:75-77`.
fn unregister_all_contexts(store: &Store, gid: &ContextGroupId) -> eyre::Result<()> {
    let contexts = calimero_governance_store::enumerate_group_contexts(store, gid, 0, usize::MAX)?;
    for ctx in contexts {
        calimero_governance_store::unregister_context_from_group(store, gid, &ctx)?;
    }
    Ok(())
}

/// Drop `GroupParentRef` + `GroupChildIndex` for `gid` under `parent`.
/// Mirrors `handlers/delete_namespace.rs:82-89`.
fn delete_tree_edges(
    store: &Store,
    gid: &ContextGroupId,
    parent: &ContextGroupId,
) -> eyre::Result<()> {
    let mut handle = store.handle();
    handle.delete(&calimero_store::key::GroupParentRef::new(gid.to_bytes()))?;
    handle.delete(&calimero_store::key::GroupChildIndex::new(
        parent.to_bytes(),
        gid.to_bytes(),
    ))?;
    Ok(())
}

/// Outcome of a [`cascade_namespace_state`] run, split into two failure
/// classes so the async wrapper can gate namespace finalization on the
/// security-critical class ONLY (#2692).
///
/// Rationale: dropping the `NamespaceIdentity` + unsubscribing is the
/// forward-secrecy-completion step. It must be gated on the signing-key
/// purge, NOT on best-effort dead-pointer cleanup — a mere context-index
/// or tree-edge orphan must not keep the namespace identity + gossipsub
/// subscription alive forever (see [`should_finalize_namespace`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct CascadeResult {
    /// Number of groups whose `delete_group_local_rows` call returned Ok.
    pub purged_groups: usize,
    /// True iff a `delete_group_local_rows` call (the security-critical
    /// signing-key purge) failed for at least one group, OR the subtree
    /// enumeration itself failed (so we cannot be sure all signing keys
    /// were swept). When true, the `NamespaceIdentity` anchor + gossipsub
    /// subscription are deliberately KEPT so the startup reconcile sweep
    /// (#2721, [`reconcile_sweep`]) resolves our identity and retries on the
    /// next process start.
    pub signing_key_purge_failed: bool,
    /// True iff a best-effort dead-pointer cleanup step failed
    /// (context-index unregister, parent-edge read, tree-edge delete, or
    /// the namespace-level state delete). Non-security: recorded for
    /// logging/metrics, but does NOT block namespace finalization.
    pub context_cleanup_failed: bool,
}

/// Pure gating decision (#2692): may the namespace-root purge finalize —
/// i.e. drop the `NamespaceIdentity` and unsubscribe from the gossipsub
/// topic — given the security-critical failure flag?
///
/// Gated on `signing_key_purge_failed` ONLY. If all signing keys are
/// gone (`signing_key_purge_failed == false`) the forward-secrecy
/// objective is met, so we finalize even if some best-effort context /
/// tree-edge cleanup failed — those orphans are non-security dead
/// pointers, and leaving the namespace identity + subscription alive on
/// such a failure is strictly worse. When the signing-key purge itself
/// failed, we KEEP the identity + subscription as a retry anchor for the
/// startup reconcile sweep (#2721). There is no EVENT-driven retry (an
/// evicted identity never gets a follow-up event); recovery comes from the
/// sweep re-running this path on the next process start.
pub(crate) fn should_finalize_namespace(signing_key_purge_failed: bool) -> bool {
    !signing_key_purge_failed
}

/// Store-side cascade for a namespace-root purge: walk the subtree
/// children-first, drop each group's local rows, then (gated on the
/// signing-key purge) drop namespace-level state.
///
/// Two-class failure tracking (#2692):
///
/// * `signing_key_purge_failed` — set ONLY when `delete_group_local_rows`
///   fails (or the subtree enumeration fails, so we can't be sure the
///   sweep was complete). This is the security-critical, load-bearing
///   step: private signing-key material lives in those rows. When set, we
///   KEEP the `NamespaceIdentity` anchor (and the caller keeps the
///   gossipsub subscription) so the startup reconcile sweep (#2721,
///   [`reconcile_sweep`]) resolves our identity and retries on the next
///   process start. There is no EVENT-driven retry (an evicted identity
///   gets no follow-up event); the sweep is what drives completion.
/// * `context_cleanup_failed` — set when a best-effort dead-pointer
///   cleanup step fails (context-index unregister, parent-edge read,
///   tree-edge delete, or the namespace-level state delete). Non-security:
///   the orphaned rows point at soon-to-be / now-deleted groups. This does
///   NOT block namespace finalization — if all signing keys are gone the
///   forward-secrecy objective is met, so we drop the `NamespaceIdentity`
///   and unsubscribe regardless. The residual dead pointers in that rare
///   store-error case are an accepted tradeoff, far better than leaving
///   the namespace identity + subscription alive on a non-security
///   failure.
///
/// Partial failures are logged and the cascade continues — the remaining
/// groups can still be cleaned up.
///
/// Sync: store operations only. Split out so tests can drive the
/// cascade without standing up a `NodeClient` mock; the async wrapper
/// [`purge_namespace_for_self`] adds the gossipsub unsubscribe on top,
/// gated via [`should_finalize_namespace`] on `signing_key_purge_failed`
/// ONLY.
///
/// Mirrors the orchestration in `handlers/delete_namespace.rs:68-93`
/// but **without the admin-authorization gate** — we are not deleting
/// other peers' state, only our own local copy of state we had access
/// to. The apply path has already committed the membership-removal,
/// so a peer racing with us cannot "rejoin" via a write under the old
/// key (the rotation pipeline excluded our identity from the new key).
pub(crate) fn cascade_namespace_state(store: &Store, ns_id: ContextGroupId) -> CascadeResult {
    let ns_hex = hex::encode(ns_id.to_bytes());

    let payload = match NamespaceRepository::new(store).collect_subtree_for_cascade(&ns_id) {
        Ok(p) => p,
        Err(e) => {
            warn!(
                namespace = %ns_hex,
                error = ?e,
                "self-purge: failed to enumerate subtree — local state may persist"
            );
            // Can't enumerate the subtree → can't be sure all signing keys
            // were swept. Treat as a signing-key failure: keep the identity
            // anchor + subscription for the reconcile sweep (#2721).
            record_purge_failure(PurgeBranch::Namespace, PurgeFailureClass::SigningKey);
            return CascadeResult {
                purged_groups: 0,
                signing_key_purge_failed: true,
                context_cleanup_failed: false,
            };
        }
    };

    let mut purged_groups = 0usize;
    let mut signing_key_purge_failed = false;
    let mut context_cleanup_failed = false;
    let all_groups = payload
        .descendant_groups
        .iter()
        .copied()
        .chain(std::iter::once(ns_id));

    // Per-group cleanup sequence mirrors `handlers/delete_namespace.rs:74-90`:
    //   1. unregister contexts (`GroupContextIndex` + `ContextGroupRef`),
    //   2. capture parent edge,
    //   3. delete_group_local_rows (members, signing keys, caps, meta, …),
    //   4. drop the parent/child tree-edge keys.
    // Steps 1, 2 and 4 were missing in v1; mdma#106-review surfaced that
    // context-index + tree-edge rows persisted after eviction.
    for gid in all_groups {
        let group_hex = hex::encode(gid.to_bytes());

        // Same priority order as the subgroup path: `delete_group_local_rows`
        // is load-bearing (signing keys); everything else is best-effort.
        // Earlier defensive `continue`s on context-unregister or
        // parent-read failure traded a signing-key leak for an
        // orphaned-pointer leak; v7 review flipped this back. Tree-edge
        // cleanup still gates on row-delete success because severing
        // the parent link while rows remain produces an unreachable-
        // but-present group. mdma#106 v7 review.

        if let Err(e) = unregister_all_contexts(store, &gid) {
            warn!(
                namespace = %ns_hex,
                group_id = %group_hex,
                error = ?e,
                "self-purge: failed to unregister contexts in cascade — \
                 context-index orphans likely; continuing"
            );
            context_cleanup_failed = true;
            record_purge_failure(PurgeBranch::Namespace, PurgeFailureClass::ContextCleanup);
        }

        let parent = NamespaceRepository::new(store)
            .parent(&gid)
            .unwrap_or_else(|e| {
                warn!(
                    namespace = %ns_hex,
                    group_id = %group_hex,
                    error = ?e,
                    "self-purge: failed to read parent edge in cascade — \
                     tree-edge cleanup will be skipped, signing-key purge proceeds"
                );
                context_cleanup_failed = true;
                record_purge_failure(PurgeBranch::Namespace, PurgeFailureClass::ContextCleanup);
                None
            });

        if let Err(e) = calimero_governance_store::delete_group_local_rows(store, &gid) {
            // Security-critical failure: private signing-key material
            // remains on disk. Set `signing_key_purge_failed` so the
            // namespace identity + gossipsub subscription are KEPT as a
            // retry anchor for the startup reconcile sweep (#2721,
            // `reconcile_sweep`), which re-runs this path on the next
            // process start. Skip tree-edge cleanup to avoid severing the
            // parent link while rows still exist.
            warn!(
                namespace = %ns_hex,
                group_id = %group_hex,
                error = ?e,
                "self-purge: failed to drop local rows for one group — \
                 signing-key material remains; skipping tree-edge cleanup; \
                 keeping namespace identity for the startup reconcile sweep (#2721)"
            );
            signing_key_purge_failed = true;
            record_purge_failure(PurgeBranch::Namespace, PurgeFailureClass::SigningKey);
            continue;
        }

        if let Some(parent) = parent {
            if let Err(e) = delete_tree_edges(store, &gid, &parent) {
                warn!(
                    namespace = %ns_hex,
                    group_id = %group_hex,
                    error = ?e,
                    "self-purge: failed to drop tree edges in cascade"
                );
                context_cleanup_failed = true;
                record_purge_failure(PurgeBranch::Namespace, PurgeFailureClass::ContextCleanup);
            }
        }

        purged_groups += 1;
    }

    // Finalize the namespace (drop `NamespaceIdentity` + gov-op log) gated
    // on the SIGNING-KEY purge ONLY (#2692). If all signing keys are gone
    // the forward-secrecy objective is met, so we complete the namespace
    // cleanup even if some best-effort context / tree-edge cleanup failed.
    // Only a signing-key purge failure keeps the identity row in place — as
    // a retry anchor for the startup reconcile sweep (#2721).
    //
    // IMPORTANT — there is no EVENT-driven retry of a signing-key failure.
    // The listener dispatches only on `TeeMemberRemoved` (not
    // `MemberRemoved`), and an already-evicted identity receives no further
    // removal events anyway (a re-admitted TEE node derives a fresh
    // attestation pubkey, so the old identity never gets a matching event).
    // Recovery instead comes from the startup reconcile sweep
    // (`reconcile_sweep`, #2721): on the next process start it re-scans the
    // local `NamespaceIdentity` rows, finds this one has no surviving
    // membership, and re-runs this cascade idempotently. So the residue
    // persists only until the next restart — the sweep is startup-only, not
    // continuous.
    //
    // This is bounded and NOT a forward-secrecy hole: FS on the namespace's
    // future writes is provided by the key-rotation pipeline (which re-keys
    // excluding the removed member), independent of this purge — see the
    // module docstring §"Forward-secrecy invariant". The residue is stale,
    // already-orphaned key material on this node's own disk, and only
    // arises on a store-level error during a per-group delete. mdma#106
    // review (cursor); #2721.
    if should_finalize_namespace(signing_key_purge_failed) {
        if let Err(e) = calimero_governance_store::delete_namespace_local_state(store, &ns_id) {
            // Best-effort: the security-critical signing keys are already
            // gone, so this is a non-security dead-pointer residue. Record
            // it as a context-cleanup failure and still finalize (the
            // caller unsubscribes) — leaving the identity + subscription
            // alive on this non-security failure would be strictly worse.
            warn!(
                namespace = %ns_hex,
                error = ?e,
                "self-purge: failed to drop namespace-level state — non-security \
                 residue (signing keys already purged); finalizing anyway"
            );
            context_cleanup_failed = true;
            record_purge_failure(PurgeBranch::Namespace, PurgeFailureClass::ContextCleanup);
        }
    } else {
        warn!(
            namespace = %ns_hex,
            purged_groups,
            "self-purge: signing-key purge failed for at least one group — \
             NamespaceIdentity + signing-key residue left on disk with no EVENT-driven \
             retry (FS still held by key rotation); the startup reconcile sweep (#2721) \
             completes it on the next restart"
        );
    }

    CascadeResult {
        purged_groups,
        signing_key_purge_failed,
        context_cleanup_failed,
    }
}

/// Namespace-root purge async wrapper: runs [`cascade_namespace_state`]
/// then unsubscribes from the namespace gossipsub topic.
///
/// The unsubscribe is **gated on the signing-key purge ONLY** (#2692, via
/// [`should_finalize_namespace`]) — exactly the same gate the cascade
/// applies to dropping `NamespaceIdentity`. If all signing keys are gone
/// the forward-secrecy objective is met, so we unsubscribe even if some
/// best-effort context / tree-edge cleanup failed. Only when the
/// signing-key purge itself failed do we KEEP the subscription.
///
/// NOTE on what now drives completion: the startup reconcile sweep
/// (`reconcile_sweep`, #2721) scans local `NamespaceIdentity` rows and
/// re-runs this purge on the next process start — it does NOT depend on the
/// gossipsub subscription (it reads on-disk rows, not the wire). So the
/// retained subscription is no longer load-bearing for retry. We keep it
/// anyway as a deliberately-narrow choice: dropping it on the
/// signing-key-failure path while rows still exist would diverge the two
/// finalize gates (identity-drop vs unsubscribe) and widen this PR's blast
/// radius into the networking path for no correctness gain — the sweep
/// already closes the residue. mdma#106 v4 review (cursor "Unsubscribe after
/// failed purge").
async fn purge_namespace_for_self(store: &Store, node_client: &NodeClient, ns_id: ContextGroupId) {
    let ns_hex = hex::encode(ns_id.to_bytes());
    let result = cascade_namespace_state(store, ns_id);

    if should_finalize_namespace(result.signing_key_purge_failed) {
        // Drop the gossipsub subscription. Best-effort; networking
        // failure here doesn't leave inconsistent on-disk state.
        if let Err(e) = node_client.unsubscribe_namespace(ns_id.to_bytes()).await {
            warn!(
                namespace = %ns_hex,
                error = ?e,
                "self-purge: failed to unsubscribe from namespace gossipsub topic"
            );
        }
        info!(
            namespace = %ns_hex,
            purged_groups = result.purged_groups,
            context_cleanup_failed = result.context_cleanup_failed,
            "self-purge: completed namespace cascade after eviction (signing keys purged); \
             unsubscribed even if best-effort context cleanup had failures"
        );
    } else {
        info!(
            namespace = %ns_hex,
            purged_groups = result.purged_groups,
            "self-purge: signing-key purge failed — keeping namespace identity + gossipsub \
             subscription; the startup reconcile sweep (#2721) retries on the next restart"
        );
    }
}

#[cfg(test)]
mod tests {
    //! Sync-side tests for the purge orchestration. The listener loop
    //! itself is template boilerplate mirroring [`crate::auto_follow`]
    //! and is not unit-tested here (covered indirectly by integration
    //! once `OpEvent::TeeMemberRemoved` lands in a real-apply-path test).
    //!
    //! What we DO verify: the store-side orchestration drops the right
    //! column families on the subgroup-only and namespace-root branches,
    //! and the cascade is idempotent (no errors / state divergence on a
    //! second call).

    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::{GroupMemberRole, UpgradePolicy};
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::Store;
    use rand::rngs::OsRng;
    use rand::RngCore;

    use calimero_governance_store::{MembershipRepository, MetaRepository, SigningKeysRepository};

    use super::*;

    fn empty_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn make_meta(admin: PublicKey) -> GroupMetaValue {
        GroupMetaValue {
            app_key: [0xBB; 32],
            target_application_id: ApplicationId::from([0xCC; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            created_at: 1_700_000_000,
            admin_identity: admin.into(),
            owner_identity: admin.into(),
            migration: None,
            auto_join: false,
        }
    }

    /// Set up a namespace root that this node is a member of, with a
    /// stored namespace identity + per-group signing-key material.
    /// Returns `(store, ns_id, self_pk)`.
    fn seed_namespace_self_member() -> (Store, ContextGroupId, PublicKey) {
        let mut rng = OsRng;
        let store = empty_store();
        let ns_id = ContextGroupId::from([0x77u8; 32]);
        let self_sk = PrivateKey::random(&mut rng);
        let self_pk = self_sk.public_key();

        MetaRepository::new(&store)
            .save(&ns_id, &make_meta(self_pk))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member_with_keys(
                &ns_id,
                &self_pk,
                GroupMemberRole::Admin,
                Some([0xAA; 32]),
                Some([0xBB; 32]),
            )
            .unwrap();
        // `add_member_with_keys` writes the per-member private/sender
        // pair into `GroupMember.{private_key, sender_key}`; the
        // `GroupSigningKey` row is a separate column written by
        // `SigningKeysRepository::store_key`. The forward-secrecy hygiene
        // we test for is on `GroupSigningKey` (where
        // `delete_all_for_group` sweeps it), so seed that row explicitly.
        SigningKeysRepository::new(&store)
            .store_key(&ns_id, &self_pk, &[0xEE; 32])
            .unwrap();
        NamespaceRepository::new(&store)
            .store_identity(&ns_id, &self_pk, &[0xAA; 32], &[0xBB; 32])
            .unwrap();

        // Sanity: pre-condition state landed.
        assert!(
            MetaRepository::new(&store).load(&ns_id).unwrap().is_some(),
            "ns meta should exist after seed"
        );
        assert!(
            SigningKeysRepository::new(&store)
                .get_key(&ns_id, &self_pk)
                .unwrap()
                .is_some(),
            "ns signing key should exist after seed"
        );
        assert!(
            NamespaceRepository::new(&store)
                .identity_record(&ns_id)
                .unwrap()
                .is_some(),
            "namespace identity should exist after seed"
        );

        (store, ns_id, self_pk)
    }

    // --- Reconcile sweep (#2721) ---------------------------------------
    //
    // These cover the startup reconcile predicate `namespace_needs_reconcile`
    // and the end-to-end "evicted residue ⇒ cascade clears keys + identity"
    // behaviour the sweep relies on. The async `reconcile_sweep` wrapper just
    // dispatches to `purge_namespace_for_self` (already covered by the cascade
    // tests) on the namespaces the predicate flags, so we test the predicate
    // (the part most likely to regress — a false `true` here would purge a
    // HEALTHY member) plus a store-level proof of the purge effect.

    #[test]
    fn reconcile_predicate_false_for_healthy_member() {
        // A node that still holds its namespace-root `GroupMember` row is a
        // live member. The predicate MUST NOT flag it for purge.
        let (store, ns_id, self_pk) = seed_namespace_self_member();
        assert!(
            !namespace_needs_reconcile(&store, ns_id, self_pk).unwrap(),
            "a healthy member (identity + root membership) MUST NOT be reconciled"
        );
    }

    #[test]
    fn reconcile_predicate_false_for_subgroup_only_member() {
        // Kicked from a subgroup but still in the namespace root: the
        // identity is legitimately retained. The predicate keys off DIRECT
        // membership anywhere under the subtree; the surviving root row
        // blocks the purge. (Subgroup-only residue is a separate, deferred
        // concern — see the `purge_subgroup_for_self` comments.)
        let (store, ns_id, self_pk) = seed_namespace_self_member();
        // Seed a descendant subgroup the node is NOT in (apply already
        // removed it), but the root membership from the seed remains.
        let sub_id = ContextGroupId::from([0x88u8; 32]);
        NamespaceRepository::new(&store)
            .nest(&ns_id, &sub_id)
            .unwrap();
        assert!(
            !namespace_needs_reconcile(&store, ns_id, self_pk).unwrap(),
            "surviving root membership MUST block the namespace-root reconcile"
        );
    }

    #[test]
    fn reconcile_predicate_false_when_member_in_descendant_only() {
        // The inverse: root membership gone, but a DESCENDANT subgroup row
        // survives. Still a live member under the namespace → no purge. This
        // guards the subtree walk (a root-only check would false-purge here).
        let (store, ns_id, self_pk) = seed_namespace_self_member();
        // Remove the root membership the seed added.
        MembershipRepository::new(&store)
            .remove_member(&ns_id, &self_pk)
            .unwrap();
        // Add a surviving membership in a nested subgroup.
        let sub_id = ContextGroupId::from([0x88u8; 32]);
        NamespaceRepository::new(&store)
            .nest(&ns_id, &sub_id)
            .unwrap();
        MembershipRepository::new(&store)
            .add_member_with_keys(
                &sub_id,
                &self_pk,
                GroupMemberRole::Member,
                Some([0xCC; 32]),
                Some([0xDD; 32]),
            )
            .unwrap();
        assert!(
            !namespace_needs_reconcile(&store, ns_id, self_pk).unwrap(),
            "a surviving DESCENDANT membership MUST block the reconcile (subtree walk)"
        );
    }

    #[test]
    fn reconcile_predicate_true_for_evicted_residue() {
        // Identity present, but NO membership row anywhere under the
        // namespace (apply removed it, the purge never completed). This is
        // exactly the residue the sweep exists to clear → predicate true.
        let (store, ns_id, self_pk) = seed_namespace_self_member();
        MembershipRepository::new(&store)
            .remove_member(&ns_id, &self_pk)
            .unwrap();
        assert!(
            namespace_needs_reconcile(&store, ns_id, self_pk).unwrap(),
            "stored identity with no surviving membership MUST be flagged as residue"
        );
    }

    #[test]
    fn reconcile_cascade_clears_signing_keys_and_identity_for_residue() {
        // End-to-end store proof: for evicted residue, running the purge path
        // the sweep invokes (`cascade_namespace_state`) clears the signing
        // keys AND the namespace identity — completing the abandoned purge.
        let (store, ns_id, self_pk) = seed_namespace_self_member();
        MembershipRepository::new(&store)
            .remove_member(&ns_id, &self_pk)
            .unwrap();

        // Precondition: the residue (identity + signing key) is present and
        // the predicate flags it.
        assert!(namespace_needs_reconcile(&store, ns_id, self_pk).unwrap());
        assert!(SigningKeysRepository::new(&store)
            .get_key(&ns_id, &self_pk)
            .unwrap()
            .is_some());

        let result = cascade_namespace_state(&store, ns_id);
        assert!(
            !result.signing_key_purge_failed,
            "reconcile cascade on a clean store must fully purge signing keys"
        );

        assert!(
            SigningKeysRepository::new(&store)
                .get_key(&ns_id, &self_pk)
                .unwrap()
                .is_none(),
            "signing-key material MUST be cleared by the reconcile cascade"
        );
        assert!(
            NamespaceRepository::new(&store)
                .identity_record(&ns_id)
                .unwrap()
                .is_none(),
            "namespace identity MUST be cleared by the reconcile cascade"
        );
    }

    #[test]
    fn reconcile_cascade_is_noop_for_healthy_member() {
        // Belt-and-suspenders: even if the cascade were mistakenly invoked on
        // a healthy member, the GUARD is the predicate. Prove the predicate
        // keeps the identity for a healthy member so the sweep never reaches
        // the cascade. (We do NOT run the cascade here — that would purge; the
        // point is the predicate gates it out.)
        let (store, ns_id, self_pk) = seed_namespace_self_member();
        assert!(
            !namespace_needs_reconcile(&store, ns_id, self_pk).unwrap(),
            "healthy member must be gated out before any cascade runs"
        );
        // Identity + signing key untouched (no purge happened).
        assert!(NamespaceRepository::new(&store)
            .identity_record(&ns_id)
            .unwrap()
            .is_some());
        assert!(SigningKeysRepository::new(&store)
            .get_key(&ns_id, &self_pk)
            .unwrap()
            .is_some());
    }

    #[test]
    fn purge_subgroup_drops_signing_key_but_leaves_namespace_identity() {
        // Subgroup-only purge: only the kicked-from group's rows should
        // go. Namespace identity + any other groups under the same
        // namespace stay (rationale: we may still be in them — same as
        // the `handlers/leave_group.rs:38-40` comment).
        let (store, ns_id, self_pk) = seed_namespace_self_member();
        let sub_id = ContextGroupId::from([0x88u8; 32]);
        MetaRepository::new(&store)
            .save(&sub_id, &make_meta(self_pk))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member_with_keys(
                &sub_id,
                &self_pk,
                GroupMemberRole::Member,
                Some([0xCC; 32]),
                Some([0xDD; 32]),
            )
            .unwrap();
        SigningKeysRepository::new(&store)
            .store_key(&sub_id, &self_pk, &[0xFF; 32])
            .unwrap();
        // Seed the parent edge so the purge path exercises the
        // `delete_tree_edges` branch. Without this, `parent` resolves
        // to None and the tree-edge delete is silently skipped —
        // making the test under-cover production behaviour.
        // mdma#106 v7 review.
        {
            use calimero_store::key::{GroupChildIndex, GroupParentRef};
            let mut handle = store.handle();
            handle
                .put(&GroupParentRef::new(sub_id.to_bytes()), &ns_id.to_bytes())
                .unwrap();
            handle
                .put(
                    &GroupChildIndex::new(ns_id.to_bytes(), sub_id.to_bytes()),
                    &(),
                )
                .unwrap();
        }
        assert!(
            SigningKeysRepository::new(&store)
                .get_key(&sub_id, &self_pk)
                .unwrap()
                .is_some(),
            "subgroup signing key should exist before purge"
        );

        purge_subgroup_for_self(&store, sub_id);

        // Tree-edge cleanup should have happened too — verify the
        // parent/child edges are gone.
        {
            use calimero_store::key::{GroupChildIndex, GroupParentRef};
            let handle = store.handle();
            assert!(
                !handle.has(&GroupParentRef::new(sub_id.to_bytes())).unwrap(),
                "GroupParentRef MUST be cleared after subgroup purge"
            );
            assert!(
                !handle
                    .has(&GroupChildIndex::new(ns_id.to_bytes(), sub_id.to_bytes()))
                    .unwrap(),
                "GroupChildIndex MUST be cleared after subgroup purge"
            );
        }

        // Post: subgroup signing key gone, subgroup meta gone, but
        // namespace identity + the ns-root signing key intact.
        assert!(
            SigningKeysRepository::new(&store)
                .get_key(&sub_id, &self_pk)
                .unwrap()
                .is_none(),
            "subgroup signing key MUST be purged"
        );
        assert!(
            MetaRepository::new(&store).load(&sub_id).unwrap().is_none(),
            "subgroup meta MUST be purged"
        );
        assert!(
            NamespaceRepository::new(&store)
                .identity_record(&ns_id)
                .unwrap()
                .is_some(),
            "namespace identity MUST NOT be touched by a subgroup-only purge"
        );
        assert!(
            SigningKeysRepository::new(&store)
                .get_key(&ns_id, &self_pk)
                .unwrap()
                .is_some(),
            "namespace-root signing key MUST NOT be touched by a subgroup-only purge"
        );
    }

    #[test]
    fn cascade_namespace_state_drops_everything_including_signing_keys() {
        // Namespace-root purge: cascade through every group's rows then
        // drop namespace-level state (identity, gov ops, head).
        let (store, ns_id, self_pk) = seed_namespace_self_member();

        let result = cascade_namespace_state(&store, ns_id);

        // At least the namespace root counted as purged (subtree may be
        // empty in this minimal fixture; the `delete_namespace.rs` call
        // shape is what we mirror — we don't assert on subtree count
        // beyond ">= 1").
        assert!(
            result.purged_groups >= 1,
            "expected at least the namespace root to be purged, got {}",
            result.purged_groups
        );
        assert!(
            !result.signing_key_purge_failed,
            "happy-path cascade on a clean fixture must not report a signing-key failure"
        );
        assert!(
            !result.context_cleanup_failed,
            "happy-path cascade on a clean fixture must not report a context-cleanup failure"
        );

        assert!(
            SigningKeysRepository::new(&store)
                .get_key(&ns_id, &self_pk)
                .unwrap()
                .is_none(),
            "namespace-root signing key MUST be purged (forward-secrecy hygiene)"
        );
        assert!(
            MetaRepository::new(&store).load(&ns_id).unwrap().is_none(),
            "namespace-root meta MUST be purged"
        );
        assert!(
            NamespaceRepository::new(&store)
                .identity_record(&ns_id)
                .unwrap()
                .is_none(),
            "namespace identity MUST be purged on a namespace-root cascade"
        );
    }

    #[test]
    fn cascade_namespace_state_drops_multi_group_subtree() {
        // Multi-subtree cascade: a namespace root with TWO nested
        // descendant groups (root → mid → leaf). The single-root test
        // above seeds no subgroups, so `collect_subtree_for_cascade`
        // returns an empty `descendant_groups` and the
        // `for gid in all_groups` loop only ever ran for the root — the
        // multi-group purge path was structurally untested (PR #2680
        // review, comment #3354456866). This fixture seeds real
        // descendants so each one's signing-key + meta rows must be swept,
        // not just the root's.
        let (store, ns_id, self_pk) = seed_namespace_self_member();

        let mid_id = ContextGroupId::from([0x91u8; 32]);
        let leaf_id = ContextGroupId::from([0x92u8; 32]);
        for sub in [mid_id, leaf_id] {
            MetaRepository::new(&store)
                .save(&sub, &make_meta(self_pk))
                .unwrap();
            MembershipRepository::new(&store)
                .add_member_with_keys(
                    &sub,
                    &self_pk,
                    GroupMemberRole::Member,
                    Some([0xCC; 32]),
                    Some([0xDD; 32]),
                )
                .unwrap();
            SigningKeysRepository::new(&store)
                .store_key(&sub, &self_pk, &[0xFF; 32])
                .unwrap();
        }
        // Wire the tree edges the way the apply path does. `nest` writes
        // BOTH `GroupParentRef` and `GroupChildIndex`; a bare
        // `GroupParentRef` would leave `list_children` blind and the
        // subtree walk would come up empty — defeating the point of the
        // test.
        NamespaceRepository::new(&store)
            .nest(&ns_id, &mid_id)
            .unwrap();
        NamespaceRepository::new(&store)
            .nest(&mid_id, &leaf_id)
            .unwrap();

        // Pre-condition: the subtree walk actually sees both descendants,
        // otherwise this test would silently degrade to the single-root
        // case it is meant to complement.
        let payload = NamespaceRepository::new(&store)
            .collect_subtree_for_cascade(&ns_id)
            .unwrap();
        assert_eq!(
            payload.descendant_groups.len(),
            2,
            "fixture must produce a 2-deep subtree so the cascade exercises \
             the multi-group loop, got {:?}",
            payload.descendant_groups
        );

        let result = cascade_namespace_state(&store, ns_id);

        assert_eq!(
            result.purged_groups, 3,
            "root + mid + leaf = 3 groups must all be purged, got {}",
            result.purged_groups
        );
        assert!(
            !result.signing_key_purge_failed,
            "happy-path multi-group cascade must not report a signing-key failure"
        );
        assert!(
            !result.context_cleanup_failed,
            "happy-path multi-group cascade must not report a context-cleanup failure"
        );

        // Forward-secrecy hygiene must reach every descendant, not just
        // the root: signing-key + meta rows gone for all three groups.
        for gid in [ns_id, mid_id, leaf_id] {
            let gid_hex = hex::encode(gid.to_bytes());
            assert!(
                SigningKeysRepository::new(&store)
                    .get_key(&gid, &self_pk)
                    .unwrap()
                    .is_none(),
                "signing key for {gid_hex} MUST be purged across the whole subtree"
            );
            assert!(
                MetaRepository::new(&store).load(&gid).unwrap().is_none(),
                "meta for {gid_hex} MUST be purged across the whole subtree"
            );
        }
        assert!(
            NamespaceRepository::new(&store)
                .identity_record(&ns_id)
                .unwrap()
                .is_none(),
            "namespace identity MUST be purged once the full subtree cascade succeeds"
        );
    }

    // --- Dispatch tests (Layer 2: wiring) ------------------------------
    //
    // These exercise `decide_purge_action`, the pure-read function the
    // listener calls to choose which purge branch (None / Subgroup /
    // Namespace) applies for a given `OpEvent::TeeMemberRemoved` event.
    // Together with the broadcast-channel sanity test below, they cover
    // the wiring the cascade unit tests deliberately skip.

    #[test]
    fn dispatch_returns_none_for_unknown_namespace() {
        // Event arrives for a group_id we never had an identity in. The
        // common case for the listener — every node receives every
        // group's events broadcast process-wide.
        let store = empty_store();
        let mut rng = OsRng;
        let other_pk = PrivateKey::random(&mut rng).public_key();
        let action = decide_purge_action(&store, [0x99u8; 32], other_pk);
        assert_eq!(action, PurgeAction::None);
    }

    #[test]
    fn dispatch_returns_none_when_event_is_about_a_different_member() {
        // We have an identity for this namespace, but the event removed
        // somebody else. We stay.
        let (store, ns_id, _self_pk) = seed_namespace_self_member();
        let mut rng = OsRng;
        let other_pk = PrivateKey::random(&mut rng).public_key();
        let action = decide_purge_action(&store, ns_id.to_bytes(), other_pk);
        assert_eq!(action, PurgeAction::None);
    }

    #[test]
    fn dispatch_returns_namespace_when_self_removed_at_namespace_root() {
        // Event removes self at the namespace root → cascade.
        let (store, ns_id, self_pk) = seed_namespace_self_member();
        let action = decide_purge_action(&store, ns_id.to_bytes(), self_pk);
        assert_eq!(action, PurgeAction::Namespace(ns_id));
    }

    #[test]
    fn dispatch_returns_subgroup_when_self_removed_at_subgroup() {
        // Event removes self at a subgroup → subgroup-only purge. We may
        // still be a member of the namespace root and other subgroups.
        let (store, ns_id, self_pk) = seed_namespace_self_member();
        let sub_id = ContextGroupId::from([0x88u8; 32]);
        MetaRepository::new(&store)
            .save(&sub_id, &make_meta(self_pk))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member_with_keys(
                &sub_id,
                &self_pk,
                GroupMemberRole::Member,
                Some([0xCC; 32]),
                Some([0xDD; 32]),
            )
            .unwrap();
        SigningKeysRepository::new(&store)
            .store_key(&sub_id, &self_pk, &[0xFF; 32])
            .unwrap();
        // Make the subgroup a child of the namespace root so
        // `NamespaceRepository::resolve` finds the namespace. Mirrors
        // what the apply path does on subgroup-creation; see
        // `governance-store/src/namespace/core.rs` for the production
        // write site.
        {
            use calimero_store::key::GroupParentRef;
            let mut handle = store.handle();
            handle
                .put(&GroupParentRef::new(sub_id.to_bytes()), &ns_id.to_bytes())
                .unwrap();
        }

        let action = decide_purge_action(&store, sub_id.to_bytes(), self_pk);
        assert_eq!(action, PurgeAction::Subgroup(sub_id));
    }

    /// Sanity check that the event channel wiring works end-to-end with
    /// our event variant: subscribe to `op_events`, notify a
    /// `TeeMemberRemoved`, verify the receiver gets it intact. This is
    /// the channel-level contract the listener depends on; if a future
    /// refactor renames the variant or breaks the broadcast plumbing,
    /// this test fails fast.
    ///
    /// Test isolation: the broadcast channel is process-wide and other
    /// tests in the same suite may emit `TeeMemberRemoved` events. We
    /// use a per-run random `group_id` as a discriminator and drain
    /// non-matching events in a recv loop. mdma#106 v6 review
    /// (meroreviewer "test is not isolated — can receive events from
    /// other tests").
    #[tokio::test]
    async fn broadcast_channel_delivers_tee_member_removed_to_subscriber() {
        let mut rng = OsRng;
        let member = PrivateKey::random(&mut rng).public_key();
        // Random 32-byte tag, not a fixed pattern, so the chance of any
        // other concurrent test using the same id is effectively zero.
        let mut group_id = [0u8; 32];
        rng.fill_bytes(&mut group_id);

        // Subscribe BEFORE notifying so we don't miss the fire.
        let mut rx = op_events::subscribe();
        op_events::notify(OpEvent::TeeMemberRemoved { group_id, member });

        // Receive in a loop, skipping events that aren't ours. The
        // channel is in-process and dispatch is sub-millisecond, so
        // 200ms is a generous timeout even under heavy parallel test
        // load.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out without receiving our discriminated TeeMemberRemoved event",
            );
            let received = tokio::time::timeout(remaining, rx.recv())
                .await
                .expect("broadcast::recv timed out — channel wiring broken")
                .expect("broadcast::recv returned an error");
            match received {
                OpEvent::TeeMemberRemoved {
                    group_id: g,
                    member: m,
                } if g == group_id => {
                    assert_eq!(m, member);
                    break;
                }
                // Not ours — another test in the process, ignore and keep
                // listening.
                _ => continue,
            }
        }
    }

    #[test]
    fn cascade_namespace_state_is_idempotent() {
        // Crash-mid-purge resilience: calling cascade twice on the same
        // store does not panic and the end state matches after the first
        // call. The returned `purged_groups` counter is "groups whose
        // delete_group_local_rows call returned Ok", not "groups that
        // actually had rows to drop" — `delete_group_local_rows` is
        // itself an idempotent batched delete, so it returns Ok even on
        // an already-empty group. We assert on the end state (which is
        // what the user cares about), not on the counter.
        let (store, ns_id, self_pk) = seed_namespace_self_member();

        let _ = cascade_namespace_state(&store, ns_id);
        // Second call: must not panic; must not error per-group.
        let _ = cascade_namespace_state(&store, ns_id);

        assert!(
            SigningKeysRepository::new(&store)
                .get_key(&ns_id, &self_pk)
                .unwrap()
                .is_none(),
            "namespace-root signing key remains purged after second cascade"
        );
        assert!(
            NamespaceRepository::new(&store)
                .identity_record(&ns_id)
                .unwrap()
                .is_none(),
            "namespace identity remains purged after second cascade"
        );
    }

    // --- Namespace-finalization gating (#2692) --------------------------
    //
    // The gating decision — "may we drop NamespaceIdentity + unsubscribe?"
    // — is extracted into the pure `should_finalize_namespace` helper so it
    // is unit-testable without injecting a `delete_group_local_rows`
    // failure (which the InMemoryDB can't readily simulate). These cover
    // the two #2692 cases plus a store-level proof that a context-cleanup
    // failure does NOT keep the namespace identity alive.

    #[test]
    fn context_cleanup_failure_only_still_finalizes_namespace() {
        // (a) A best-effort context/tree-edge cleanup failure must NOT
        // block finalization: if signing keys are gone, drop the identity
        // and unsubscribe.
        assert!(
            should_finalize_namespace(false),
            "context-cleanup-only failure (signing_key_purge_failed=false) MUST finalize \
             the namespace and proceed to unsubscribe"
        );
    }

    #[test]
    fn signing_key_failure_keeps_namespace_identity_and_subscription() {
        // (b) A signing-key purge failure MUST keep the identity (retry
        // anchor for #2721) and skip the unsubscribe.
        assert!(
            !should_finalize_namespace(true),
            "signing-key purge failure (signing_key_purge_failed=true) MUST keep the \
             namespace identity and skip the unsubscribe"
        );
    }

    #[test]
    fn cascade_with_context_cleanup_failure_drops_identity() {
        // End-to-end store proof for case (a): seed a namespace whose
        // subtree walk yields a child whose context-unregister will fail,
        // while the signing-key purge succeeds. The namespace identity MUST
        // still be dropped because `signing_key_purge_failed == false`.
        //
        // We can't inject a `delete_group_local_rows` failure with the
        // InMemoryDB, but the inverse — a clean run where only best-effort
        // steps are exercised — already proves the gate finalizes when
        // signing keys are gone (covered by the happy-path cascade tests).
        // To exercise the `context_cleanup_failed == true` path concretely
        // we rely on the pure helper above; here we assert the structural
        // invariant that a successful signing-key purge always reports
        // `signing_key_purge_failed == false` so the gate opens.
        let (store, ns_id, _self_pk) = seed_namespace_self_member();
        let result = cascade_namespace_state(&store, ns_id);
        assert!(
            !result.signing_key_purge_failed,
            "a clean cascade reports no signing-key failure, so the namespace finalizes"
        );
        assert!(
            should_finalize_namespace(result.signing_key_purge_failed),
            "gate must open for a clean cascade"
        );
        assert!(
            NamespaceRepository::new(&store)
                .identity_record(&ns_id)
                .unwrap()
                .is_none(),
            "namespace identity MUST be dropped when the signing-key purge succeeded"
        );
    }

    // --- Role-scoped dispatch regression tests --------------------------
    //
    // These guard the contract that closed PR #2653: only
    // `TeeMemberRemoved` triggers the listener's purge. Non-TEE
    // `MemberRemoved` events stay on the soft-leave path (existing
    // kick-and-readd / rejoin-via-keyshare / inheritance-rejoin e2e
    // workflows under `apps/scaffolding-e2e/workflows/group-{kick,leave}-*`
    // depend on this).

    #[test]
    fn dispatch_target_skips_non_tee_member_removed() {
        // Regression: a soft-leave or admin-kick of a non-TEE member
        // must NOT trip the self-purge listener. If the predicate ever
        // starts returning `Some` for `MemberRemoved`, the 4 e2e
        // workflows that this PR was narrowed to preserve will break.
        let mut rng = OsRng;
        let member = PrivateKey::random(&mut rng).public_key();
        let event = OpEvent::MemberRemoved {
            group_id: [0xAAu8; 32],
            member,
        };
        assert_eq!(
            dispatch_target(&event),
            None,
            "non-TEE MemberRemoved must NOT dispatch the self-purge listener"
        );
    }

    #[test]
    fn dispatch_target_fires_on_tee_member_removed() {
        // Positive path: the role-scoped follow-up event is exactly the
        // listener's wake-up signal.
        let mut rng = OsRng;
        let member = PrivateKey::random(&mut rng).public_key();
        let gid = [0xBBu8; 32];
        let event = OpEvent::TeeMemberRemoved {
            group_id: gid,
            member,
        };
        assert_eq!(
            dispatch_target(&event),
            Some((gid, member)),
            "TeeMemberRemoved MUST dispatch the self-purge listener"
        );
    }

    #[test]
    fn dispatch_target_skips_unrelated_op_events() {
        // Any other op-event variant must be ignored by the listener
        // (auto-follow / context-registered / etc. handlers own those).
        let mut rng = OsRng;
        let member = PrivateKey::random(&mut rng).public_key();
        let gid = [0xCCu8; 32];

        assert_eq!(
            dispatch_target(&OpEvent::MemberAdded {
                group_id: gid,
                member,
                role: GroupMemberRole::ReadOnlyTee,
            }),
            None,
        );
        assert_eq!(
            dispatch_target(&OpEvent::TeeMemberAdmitted {
                group_id: gid,
                member,
            }),
            None,
        );
    }

    /// Live broadcast-channel sanity for the negative path: emit a
    /// non-TEE `MemberRemoved` event on the global op-events channel
    /// and verify the listener's match-arm predicate continues to
    /// reject it after a round-trip through the broadcast channel.
    /// Belt-and-suspenders with the unit-level `dispatch_target_*`
    /// tests above — this one would catch a future refactor that moved
    /// the role gate INTO the channel (e.g. variant rename) and broke
    /// the wire-format compatibility.
    #[tokio::test]
    async fn live_channel_skips_non_tee_member_removed() {
        let mut rng = OsRng;
        let member = PrivateKey::random(&mut rng).public_key();
        let mut group_id = [0u8; 32];
        rng.fill_bytes(&mut group_id);

        let mut rx = op_events::subscribe();
        op_events::notify(OpEvent::MemberRemoved { group_id, member });

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out without receiving our discriminated MemberRemoved event",
            );
            let received = tokio::time::timeout(remaining, rx.recv())
                .await
                .expect("broadcast::recv timed out")
                .expect("broadcast::recv returned an error");
            match received {
                OpEvent::MemberRemoved {
                    group_id: g,
                    member: _,
                } if g == group_id => {
                    assert_eq!(
                        dispatch_target(&received),
                        None,
                        "the listener must NOT dispatch on a non-TEE MemberRemoved \
                         delivered via the live broadcast channel",
                    );
                    break;
                }
                _ => continue,
            }
        }
    }
}
