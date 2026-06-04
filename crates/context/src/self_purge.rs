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
use calimero_governance_store::op_events::{self, OpEvent};
use calimero_governance_store::NamespaceRepository;

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

    loop {
        let event = match rx.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                // Missed events: when this happens, a future eviction event
                // (or a process restart that re-runs `run`) will pick up the
                // skipped state — but for the immediate event that was
                // skipped, the evicted membership row is already gone from
                // the local store (apply committed before notify) while the
                // signing-key + gov-op rows linger. The next eviction event
                // that DOES land for this identity will sweep them
                // incidentally because the purge helpers are idempotent and
                // namespace-scoped. We accept this as the documented
                // "soft-leave residue" failure mode rather than adding a
                // startup reconcile loop; see ADR 0002 § "Failure modes we
                // accept".
                warn!(
                    skipped,
                    "self-purge subscriber lagged; some events were dropped — \
                     residual local state may persist until the next eviction \
                     or process restart"
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
             subgroup-only purge; manual cleanup or startup-reconcile \
             follow-up needed; see ADR 0002)"
        );
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
            // to drive a retry, and the broader startup-reconcile sweep
            // is the deferred follow-up tracked by ADR 0002. The leak is
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
                 subgroup-only purge; see ADR 0002 startup-reconcile follow-up)"
            );
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

/// Outcome of a [`cascade_namespace_state`] run. Used by the async
/// wrapper to decide whether to also unsubscribe from gossipsub —
/// keeping the subscription on partial failure preserves the retry
/// path (the next `MemberRemoved` event for this namespace must reach
/// us via gossip).
#[derive(Debug, Clone, Copy)]
pub(crate) struct CascadeResult {
    /// Number of groups whose `delete_group_local_rows` call returned Ok.
    pub purged_groups: usize,
    /// True iff every per-group step AND the namespace-level state delete
    /// succeeded. False if any step failed (in which case the namespace
    /// identity was deliberately kept on disk to allow the next event
    /// to resolve us and retry).
    pub all_succeeded: bool,
}

/// Store-side cascade for a namespace-root purge: walk the subtree
/// children-first, drop each group's local rows, then drop namespace-
/// level state.
///
/// Partial failures are logged and the cascade continues — the
/// remaining groups can still be cleaned up, and the next eviction
/// event (or restart) will retry the residue idempotently. On any
/// per-group failure the namespace-level state delete is skipped so
/// the next event can resolve our identity and try again.
///
/// Sync: store operations only. Split out so tests can drive the
/// cascade without standing up a `NodeClient` mock; the async wrapper
/// [`purge_namespace_for_self`] adds the gossipsub unsubscribe on top
/// (gated on `all_succeeded`).
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
            return CascadeResult {
                purged_groups: 0,
                all_succeeded: false,
            };
        }
    };

    let mut purged_groups = 0usize;
    let mut any_group_failed = false;
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
            any_group_failed = true;
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
                any_group_failed = true;
                None
            });

        if let Err(e) = calimero_governance_store::delete_group_local_rows(store, &gid) {
            // Signing-key material remains. The cascade has a retry
            // surface (next MemberRemoved event for this identity), so
            // `any_group_failed = true` also keeps the NamespaceIdentity
            // anchor in place for that retry. Skip tree-edge cleanup to
            // avoid severing the parent link while rows still exist.
            warn!(
                namespace = %ns_hex,
                group_id = %group_hex,
                error = ?e,
                "self-purge: failed to drop local rows for one group — \
                 skipping tree-edge cleanup; next event will retry"
            );
            any_group_failed = true;
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
                any_group_failed = true;
            }
        }

        purged_groups += 1;
    }

    // Only drop namespace-level state if every per-group purge succeeded.
    // Otherwise we'd delete `NamespaceIdentity` while leaving stranded
    // `GroupSigningKey` material — and future `MemberRemoved` events for
    // the same identity would hit `decide_purge_action`, fail to find the
    // identity (because we just deleted it), return `PurgeAction::None`,
    // and never sweep the residue. Keeping the identity around on partial
    // failure preserves the next-event retry path, at the cost of leaving
    // the namespace gov-op log and the identity row in place. The retry
    // is idempotent — succeeding next time then clears everything.
    // mdma#106 review (cursor).
    let mut all_succeeded = !any_group_failed;
    if any_group_failed {
        warn!(
            namespace = %ns_hex,
            purged_groups,
            "self-purge: per-group cascade had failures — leaving NamespaceIdentity in \
             place so the next MemberRemoved event can resolve our identity and retry"
        );
    } else if let Err(e) = calimero_governance_store::delete_namespace_local_state(store, &ns_id) {
        warn!(
            namespace = %ns_hex,
            error = ?e,
            "self-purge: failed to drop namespace-level state"
        );
        all_succeeded = false;
    }

    CascadeResult {
        purged_groups,
        all_succeeded,
    }
}

/// Namespace-root purge async wrapper: runs [`cascade_namespace_state`]
/// then unsubscribes from the namespace gossipsub topic.
///
/// The unsubscribe is **gated on `all_succeeded`** — on partial failure
/// the cascade deliberately keeps `NamespaceIdentity` around so that a
/// future `MemberRemoved` event can resolve our identity and retry. But
/// that future event must reach us, and namespace governance events
/// arrive via the namespace gossipsub topic; unsubscribing here would
/// silently break the retry path. mdma#106 v4 review (cursor "Unsubscribe
/// after failed purge").
async fn purge_namespace_for_self(store: &Store, node_client: &NodeClient, ns_id: ContextGroupId) {
    let ns_hex = hex::encode(ns_id.to_bytes());
    let result = cascade_namespace_state(store, ns_id);

    if result.all_succeeded {
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
            "self-purge: completed namespace cascade after eviction"
        );
    } else {
        info!(
            namespace = %ns_hex,
            purged_groups = result.purged_groups,
            "self-purge: namespace cascade had failures — keeping gossipsub subscription \
             so the next MemberRemoved event can drive a retry"
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
            result.all_succeeded,
            "happy-path cascade on a clean fixture should report all_succeeded=true"
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
            result.all_succeeded,
            "happy-path multi-group cascade should report all_succeeded=true"
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
