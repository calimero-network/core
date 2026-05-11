//! Per-group deny-list for removed members.
//!
//! Drops state deltas from a member before the cross-DAG membership check
//! runs. The cross-DAG check (`membership_status_at`) is the authoritative
//! enforcement — a removed member's deltas are rejected by it regardless.
//! This module is a cheap early-rejection layer that:
//!
//! 1. **Saves work**: avoids the governance-pending drain pass + the
//!    membership lookup + the prefix walk for traffic from peers we've
//!    already removed. The hot path becomes a single store-key existence
//!    check.
//! 2. **Defense-in-depth**: surfaces removed-peer activity at the entry
//!    point with a dedicated log line that's easier to correlate to a
//!    removal op than `cross-DAG check: rejecting state delta — author is
//!    not a member`.
//!
//! Per-group rather than per-peer-id: the same identity can be a member of
//! multiple groups, and connection-level (libp2p) gating on peer-id would
//! drop legitimate traffic for the groups they still belong to. Filtering
//! at the gossipsub-message-receive layer keyed by `(group_id, identity)`
//! is the right granularity — each context has its own gossip topic, so
//! the deny set is scoped to exactly the contexts where the member was
//! removed.
//!
//! Entries are added when `MemberRemoved` / `MemberLeft` apply, and
//! cleared when `MemberAdded` / `MemberJoinedViaTeeAttestation` apply for
//! the same `(group_id, identity)` pair. Add → Remove → Add cycles end
//! with the entry cleared — the deny-list is a derived view of "currently
//! not a member," not an audit log.

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupDeniedMember, GROUP_DENIED_MEMBER_PREFIX};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::collect_keys_with_prefix;

/// Mark `member` as denied for `group_id`. Idempotent — calling this on an
/// already-denied member is a no-op (RocksDB put on an existing key just
/// overwrites the same `()` marker).
///
/// **Caller contract:** invoke only after the corresponding membership-
/// removal apply (`MemberRemoved` / `MemberLeft`) has run, so the
/// deny-list view stays consistent with the materialized member set. The
/// primitive itself does not verify removal — calling it on a current
/// member produces an inconsistent state (denied at the receive filter
/// but still resolves as a member in governance queries). Current call
/// sites are inside `apply_group_op_mutations` immediately after the
/// `remove_group_member` write, which is the only safe placement.
pub fn mark_denied(store: &Store, group_id: &ContextGroupId, member: &PublicKey) -> EyreResult<()> {
    let key = GroupDeniedMember::new(group_id.to_bytes(), *member);
    let mut handle = store.handle();
    handle
        .put(&key, &())
        .map_err(|e| eyre::eyre!("mark_denied: {e}"))?;
    Ok(())
}

/// Clear `member`'s deny-list entry for `group_id`. Idempotent — calling
/// this on a non-denied member is a no-op. Invoked when a previously-
/// removed member is re-added.
pub fn clear_denied(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    let key = GroupDeniedMember::new(group_id.to_bytes(), *member);
    let mut handle = store.handle();
    handle
        .delete(&key)
        .map_err(|e| eyre::eyre!("clear_denied: {e}"))?;
    Ok(())
}

/// Check whether `member` is currently denied for `group_id`.
///
/// Hot-path callers (receive-side state-delta filter) call this on every
/// incoming state delta for a group context. O(1) key lookup.
pub fn is_denied(store: &Store, group_id: &ContextGroupId, member: &PublicKey) -> EyreResult<bool> {
    let key = GroupDeniedMember::new(group_id.to_bytes(), *member);
    let handle = store.handle();
    handle.has(&key).map_err(|e| eyre::eyre!("is_denied: {e}"))
}

/// Remove every deny-list entry under `group_id`. Used during group
/// teardown (`delete_group_local_rows`) so the deny set doesn't outlive
/// the group it describes.
pub fn clear_all_denied(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let gid = group_id.to_bytes();
    // The seek start key uses `[0u8; 32]` as the identity component —
    // the lexicographic minimum of the 32-byte identity space, so no
    // valid `PublicKey` can sort before it. RocksDB uses byte-wise
    // comparison, so a forward iterator seeded here visits every
    // `GroupDeniedMember` row whose `group_id` matches `gid`. This is
    // the same scan-from-minimum convention used by
    // `delete_all_member_capabilities` and the other per-group sweep
    // helpers in this module.
    let keys = collect_keys_with_prefix(
        store,
        GroupDeniedMember::new(gid, PublicKey::from([0u8; 32])),
        GROUP_DENIED_MEMBER_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let mut handle = store.handle();
    for key in keys {
        handle
            .delete(&key)
            .map_err(|e| eyre::eyre!("clear_all_denied: {e}"))?;
    }
    Ok(())
}
