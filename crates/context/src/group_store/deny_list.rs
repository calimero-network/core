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

/// Typed Repository for the per-group deny-list.
///
/// See module-level docs for the design rationale and apply-path
/// contract. The Repository is a thin layer over `&Store`; it doesn't
/// enforce the "denied implies removed" invariant — that's the
/// caller's responsibility, asserted in dev/test via [`Self::mark`]'s
/// `debug_assert!`.
///
/// Issue #2303 / epic #2300.
pub struct DenyListRepository<'a> {
    store: &'a Store,
}

impl<'a> DenyListRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Mark `member` as denied for `group_id`. Idempotent — calling this
    /// on an already-denied member is a no-op (RocksDB put on an
    /// existing key just overwrites the same `()` marker).
    ///
    /// **Caller contract:** invoke only after the corresponding
    /// membership-removal apply (`MemberRemoved` / `MemberLeft`) has
    /// run, so the deny-list view stays consistent with the
    /// materialized member set. The primitive itself does not verify
    /// removal — calling it on a current member produces an
    /// inconsistent state (denied at the receive filter but still
    /// resolves as a member in governance queries). Current call sites
    /// are inside `apply_group_op_mutations` immediately after the
    /// `remove_group_member` write, which is the only safe placement.
    ///
    /// A `debug_assert!` enforces the contract in dev / test builds.
    /// It is compiled out in release so the production cost is zero —
    /// the assertion exists to catch misuse during development.
    pub fn mark(&self, group_id: &ContextGroupId, member: &PublicKey) -> EyreResult<()> {
        debug_assert!(
            !super::membership::has_direct_group_member(self.store, group_id, member)
                .unwrap_or(true),
            "DenyListRepository::mark: member {member:?} is still in the materialized set \
             for group {group_id:?} — callers must invoke remove_group_member first \
             (see caller contract)"
        );
        let key = GroupDeniedMember::new(group_id.to_bytes(), *member);
        let mut handle = self.store.handle();
        handle
            .put(&key, &())
            .map_err(|e| eyre::eyre!("DenyListRepository::mark: {e}"))?;
        Ok(())
    }

    /// Clear `member`'s deny-list entry for `group_id`. Idempotent —
    /// calling this on a non-denied member is a no-op. Invoked when a
    /// previously-removed member is re-added.
    pub fn clear(&self, group_id: &ContextGroupId, member: &PublicKey) -> EyreResult<()> {
        let key = GroupDeniedMember::new(group_id.to_bytes(), *member);
        let mut handle = self.store.handle();
        handle
            .delete(&key)
            .map_err(|e| eyre::eyre!("DenyListRepository::clear: {e}"))?;
        Ok(())
    }

    /// Check whether `member` is currently denied for `group_id`.
    ///
    /// Hot-path callers (receive-side state-delta filter) call this on
    /// every incoming state delta for a group context. O(1) key lookup.
    pub fn is_denied(&self, group_id: &ContextGroupId, member: &PublicKey) -> EyreResult<bool> {
        let key = GroupDeniedMember::new(group_id.to_bytes(), *member);
        let handle = self.store.handle();
        handle
            .has(&key)
            .map_err(|e| eyre::eyre!("DenyListRepository::is_denied: {e}"))
    }

    /// Check whether `author` is denied for the group that owns
    /// `context_id`. Returns `Ok(false)` when the context isn't
    /// registered to any group (nothing to deny on) — group-less
    /// contexts skip the deny-list layer entirely. Encapsulates the
    /// two-step `get_group_for_context` → `is_denied` lookup so callers
    /// (e.g. the state-delta handler) don't have to reach into the
    /// group-id resolution.
    pub fn is_author_denied_for_context(
        &self,
        context_id: &calimero_primitives::context::ContextId,
        author: &PublicKey,
    ) -> EyreResult<bool> {
        let Some(group_id) = super::contexts::get_group_for_context(self.store, context_id)? else {
            return Ok(false);
        };
        self.is_denied(&group_id, author)
    }

    /// Remove every deny-list entry under `group_id`. Used during group
    /// teardown (`delete_group_local_rows`) so the deny set doesn't
    /// outlive the group it describes.
    pub fn clear_all_for_group(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let gid = group_id.to_bytes();
        // The seek start key uses `[0u8; 32]` as the identity component —
        // the lexicographic minimum of the 32-byte identity space, so no
        // valid `PublicKey` can sort before it. RocksDB uses byte-wise
        // comparison, so a forward iterator seeded here visits every
        // `GroupDeniedMember` row whose `group_id` matches `gid`. Same
        // scan-from-minimum convention as `delete_all_member_capabilities`.
        let keys = collect_keys_with_prefix(
            self.store,
            GroupDeniedMember::new(gid, PublicKey::from([0u8; 32])),
            GROUP_DENIED_MEMBER_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let mut handle = self.store.handle();
        for key in keys {
            handle
                .delete(&key)
                .map_err(|e| eyre::eyre!("DenyListRepository::clear_all_for_group: {e}"))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Deprecated free-function wrappers.
// ---------------------------------------------------------------------------

#[deprecated(note = "use DenyListRepository::new(store).mark(...)")]
pub fn mark_denied(store: &Store, group_id: &ContextGroupId, member: &PublicKey) -> EyreResult<()> {
    DenyListRepository::new(store).mark(group_id, member)
}

#[deprecated(note = "use DenyListRepository::new(store).clear(...)")]
pub fn clear_denied(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    DenyListRepository::new(store).clear(group_id, member)
}

#[deprecated(note = "use DenyListRepository::new(store).is_denied(...)")]
pub fn is_denied(store: &Store, group_id: &ContextGroupId, member: &PublicKey) -> EyreResult<bool> {
    DenyListRepository::new(store).is_denied(group_id, member)
}

#[deprecated(note = "use DenyListRepository::new(store).is_author_denied_for_context(...)")]
pub fn is_author_denied_for_context(
    store: &Store,
    context_id: &calimero_primitives::context::ContextId,
    author: &PublicKey,
) -> EyreResult<bool> {
    DenyListRepository::new(store).is_author_denied_for_context(context_id, author)
}

#[deprecated(note = "use DenyListRepository::new(store).clear_all_for_group(...)")]
pub fn clear_all_denied(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    DenyListRepository::new(store).clear_all_for_group(group_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_store::test_fixtures::{test_group_id, test_store};

    #[test]
    fn is_denied_returns_false_when_unset() {
        let store = test_store();
        let repo = DenyListRepository::new(&store);
        let pk = PublicKey::from([0x01; 32]);
        assert!(!repo.is_denied(&test_group_id(), &pk).unwrap());
    }

    #[test]
    fn mark_then_is_denied_round_trip() {
        let store = test_store();
        let repo = DenyListRepository::new(&store);
        let gid = test_group_id();
        let pk = PublicKey::from([0x01; 32]);

        // Skip the debug_assert (member must be absent from materialized set
        // — already true since we never added them) and call mark directly.
        repo.mark(&gid, &pk).unwrap();
        assert!(repo.is_denied(&gid, &pk).unwrap());
    }

    #[test]
    fn clear_after_mark_returns_to_not_denied() {
        let store = test_store();
        let repo = DenyListRepository::new(&store);
        let gid = test_group_id();
        let pk = PublicKey::from([0x01; 32]);

        repo.mark(&gid, &pk).unwrap();
        repo.clear(&gid, &pk).unwrap();
        assert!(!repo.is_denied(&gid, &pk).unwrap());
    }

    #[test]
    fn clear_is_idempotent_when_unset() {
        let store = test_store();
        let repo = DenyListRepository::new(&store);
        let pk = PublicKey::from([0x01; 32]);
        // Clearing an absent entry must succeed silently.
        repo.clear(&test_group_id(), &pk).unwrap();
    }

    #[test]
    fn mark_is_idempotent_on_already_denied() {
        let store = test_store();
        let repo = DenyListRepository::new(&store);
        let gid = test_group_id();
        let pk = PublicKey::from([0x01; 32]);

        repo.mark(&gid, &pk).unwrap();
        repo.mark(&gid, &pk).unwrap();
        assert!(repo.is_denied(&gid, &pk).unwrap());
    }

    #[test]
    fn clear_all_for_group_clears_only_that_group() {
        let store = test_store();
        let repo = DenyListRepository::new(&store);
        let gid_a = test_group_id();
        let gid_b = ContextGroupId::from([0xBB; 32]);
        let pk = PublicKey::from([0x01; 32]);

        repo.mark(&gid_a, &pk).unwrap();
        repo.mark(&gid_b, &pk).unwrap();

        repo.clear_all_for_group(&gid_a).unwrap();

        assert!(!repo.is_denied(&gid_a, &pk).unwrap());
        assert!(repo.is_denied(&gid_b, &pk).unwrap());
    }
}
