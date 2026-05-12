use calimero_context_config::types::{ContextGroupId, GovernancePosition};
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{ContextMeta, GroupMeta, GroupMetaValue, GROUP_META_PREFIX};
use calimero_store::Store;
use eyre::{eyre, Result as EyreResult};
use sha2::{Digest, Sha256};

use super::{
    collect_keys_with_prefix_paginated, enumerate_group_contexts, list_group_members,
    resolve_namespace, NamespaceDagService,
};

pub fn load_group_meta(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<GroupMetaValue>> {
    let handle = store.handle();
    let key = GroupMeta::new(group_id.to_bytes());
    let value = handle.get(&key)?;
    Ok(value)
}

pub fn save_group_meta(
    store: &Store,
    group_id: &ContextGroupId,
    meta: &GroupMetaValue,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMeta::new(group_id.to_bytes());
    handle.put(&key, meta)?;
    Ok(())
}

pub fn delete_group_meta(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMeta::new(group_id.to_bytes());
    handle.delete(&key)?;
    Ok(())
}

pub fn enumerate_all_groups(
    store: &Store,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<([u8; 32], GroupMetaValue)>> {
    let keys = collect_keys_with_prefix_paginated(
        store,
        GroupMeta::new([0u8; 32]),
        GROUP_META_PREFIX,
        |_| true,
        offset,
        limit,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();

    for key in keys {
        let Some(meta) = handle.get(&key)? else {
            continue;
        };
        results.push((key.group_id(), meta));
    }

    Ok(results)
}

/// Compute a deterministic SHA-256 hash of the group's authorization-relevant state.
///
/// Covers members (sorted by public key) + roles + admin identity + owner identity +
/// target application. This hash is embedded in each SignedGroupOp to ensure ops can
/// only apply against the exact state they were signed against, preventing divergence
/// from concurrent ops.
///
/// `owner_identity` is part of the hash because it gates a real authorization decision:
/// `TransferOwnership`, `GroupDelete`, and the `CannotRemoveOwner` check on
/// `MemberRemoved` all branch on the current owner. Without including it, two ops
/// signed before and after a `TransferOwnership` would compute the same state hash and
/// the divergence-prevention check would fail to detect that ownership changed.
pub fn compute_group_state_hash(store: &Store, group_id: &ContextGroupId) -> EyreResult<[u8; 32]> {
    let meta = load_group_meta(store, group_id)?
        .ok_or_else(|| eyre::eyre!("group not found for state hash computation"))?;

    let mut members = list_group_members(store, group_id, 0, usize::MAX)?;
    members.sort_by(|a, b| a.0.cmp(&b.0));

    hash_group_state(group_id, &meta, &members)
}

/// Single source of truth for the group state hash byte layout. Both
/// `compute_group_state_hash` (post-apply, reads real store state) and
/// `compute_group_state_hash_after_remove` (pre-apply simulation) feed
/// their prepared sorted-member list here so any change to the hash
/// format is structurally guaranteed to apply to both paths — no
/// silent sign/verify divergence from a one-sided update.
///
/// **Caller contract**: `members` MUST be sorted by `PublicKey` byte
/// ordering. The hash is order-sensitive; an unsorted slice produces a
/// different digest for the same logical set and breaks convergence.
fn hash_group_state(
    group_id: &ContextGroupId,
    meta: &GroupMetaValue,
    members_sorted: &[(PublicKey, GroupMemberRole)],
) -> EyreResult<[u8; 32]> {
    // Sorted-input contract enforced in dev / test, compiled out in
    // release. Catches a caller that forgets to sort before
    // delegating — the same shape used by `diff_sorted_context_hashes`.
    debug_assert!(
        members_sorted
            .windows(2)
            .all(|w| AsRef::<[u8]>::as_ref(&w[0].0) < AsRef::<[u8]>::as_ref(&w[1].0)),
        "hash_group_state: members must be strictly sorted by PublicKey byte order"
    );
    let mut hasher = Sha256::new();
    hasher.update(group_id.to_bytes());
    hasher.update(AsRef::<[u8]>::as_ref(&meta.admin_identity));
    hasher.update(AsRef::<[u8]>::as_ref(&meta.owner_identity));
    hasher.update(meta.target_application_id.as_ref());
    for (pk, role) in members_sorted {
        hasher.update(AsRef::<[u8]>::as_ref(pk));
        let role_bytes =
            borsh::to_vec(role).map_err(|e| eyre!("role serialization failed: {e}"))?;
        hasher.update(&role_bytes);
    }
    Ok(hasher.finalize().into())
}

/// Return the group state hash that would result if `removed_member`
/// were dropped from the group's member set. Pure simulation: reads
/// the current materialized state, removes the named identity from the
/// sorted-by-pubkey member list in-memory, and hashes — mirrors what
/// [`compute_group_state_hash`] would compute after a `MemberRemoved`
/// or `MemberLeft` apply.
///
/// Used at sign time so the admin (or leaver) can populate the
/// `expected_group_state_hash` field on `MemberRemoved` / `MemberLeft`
/// before the apply runs locally. Receivers compute the real hash
/// after their own apply and compare; mismatch surfaces membership-row
/// divergence.
///
/// Idempotent on a non-member input — if `removed_member` isn't in the
/// set, the result is the same as `compute_group_state_hash` on the
/// current state. The op apply path bails on non-members independently;
/// this helper just computes deterministically over whatever set it
/// finds.
pub fn compute_group_state_hash_after_remove(
    store: &Store,
    group_id: &ContextGroupId,
    removed_member: &PublicKey,
) -> EyreResult<[u8; 32]> {
    let meta = load_group_meta(store, group_id)?
        .ok_or_else(|| eyre!("group not found for state hash computation"))?;

    let mut members = list_group_members(store, group_id, 0, usize::MAX)?;
    members.retain(|(pk, _role)| pk != removed_member);
    members.sort_by(|a, b| a.0.cmp(&b.0));

    hash_group_state(group_id, &meta, &members)
}

/// Snapshot the current CRDT root hash for every context registered
/// under `group_id`. Returned sorted by `context_id` for deterministic
/// op-content hashing (the result lands inside a signed governance op
/// whose content hash is the dedup key).
///
/// `MemberRemoved` / `MemberLeft` don't directly mutate per-context
/// CRDT state, so this is simply the admin's view of "what these
/// contexts look like right now" at sign time. Receivers compare
/// against their own context roots after applying the removal; a
/// divergent root means the receiver applied legitimate pre-removal
/// state-DAG deltas from the now-removed member that admin's view
/// didn't include — the partition-window case the anchor-sync
/// reconcile path will heal.
///
/// Contexts whose `ContextMeta` row is missing (registered in the
/// group index but not yet materialized — e.g. fresh node that hasn't
/// joined yet) are skipped, not errored. Hashing what isn't there
/// would put zero bytes in the snapshot and force a divergence on
/// every receiver that has the context materialized.
///
/// **Asymmetric skip behavior between signer and receiver — by
/// design.** The signer's call (at op-construction time) writes
/// whichever contexts it has materialized into the signed claim. The
/// receiver's call (during apply-time verification) reads its own
/// materialized set. A context the signer had but the receiver
/// doesn't appears as "only in expected" in the diff — and that's
/// exactly the partition-window signal the anchor-sync reconcile
/// path consumes to heal. On freshly-joined receivers many contexts
/// may be unmaterialized and produce this signal in bulk; the
/// per-context debug log in `diff_sorted_context_hashes` lets
/// operators distinguish bootstrap catchup from real divergence.
pub fn snapshot_context_state_hashes(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(ContextId, [u8; 32])>> {
    let context_ids = enumerate_group_contexts(store, group_id, 0, usize::MAX)?;
    let handle = store.handle();
    let mut entries = Vec::new();
    for context_id in context_ids {
        let key = ContextMeta::new(context_id);
        if let Some(meta) = handle.get(&key)? {
            entries.push((context_id, meta.root_hash));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

/// Build the current cross-DAG cut for `group_id` — pre-apply group
/// state hash bundled with the namespace governance DAG heads at this
/// moment. Used as the `cut` field in `MemberRemoved` / `MemberLeft`
/// ops so receivers' apply-time membership check evaluates against the
/// same descend-from boundary the signer signed against (not against
/// each receiver's possibly-different local namespace heads).
///
/// The two store reads (DAG heads and group state hash) are NOT
/// atomic. Callers must invoke this from a serializing context where
/// no concurrent namespace governance op can land between the reads —
/// in practice the `ContextManager` actor that owns all governance
/// publishing. A reader running outside that serialization could
/// receive a position whose `governance_dag_heads` and
/// `group_state_hash` describe different points in time. The receiver
/// would detect the resulting hash mismatch in
/// `verify_post_apply_state_hashes` and trip a spurious divergence
/// warning, but no state is adopted from the mismatch, so the worst
/// case is operator noise.
///
/// Once the cut is built it travels intact in the signed op — there's
/// no second read by receivers, so the only race is on the signer's
/// side.
pub fn build_governance_cut(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<GovernancePosition> {
    let namespace_id = resolve_namespace(store, group_id)?;
    let dag = NamespaceDagService::new(store, namespace_id.to_bytes());
    let governance_dag_heads = dag.read_head_record()?.parent_hashes;
    let group_state_hash = compute_group_state_hash(store, group_id)?;
    GovernancePosition::new(*group_id, group_state_hash, governance_dag_heads)
        .map_err(|e| eyre!("invalid governance position at sign time: {e}"))
}
