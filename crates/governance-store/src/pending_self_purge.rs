//! Durable pending-self-purge marker store (#2721).
//!
//! A marker keyed by `namespace_id` records that THIS node was confirmed
//! TEE-self-evicted from the namespace and its local-state cascade purge is
//! in flight or incomplete. The marker is the role/intent gate for the
//! startup reconcile sweep: the sweep completes ONLY marked namespaces, so
//! it structurally cannot false-purge either of the two look-alike states
//! that confused the previous role-blind full-scan reconcile:
//!
//! 1. **Pending join** — the join path writes the `NamespaceIdentity` BEFORE
//!    the joiner's `GroupMember` row materializes. A restart mid-join leaves
//!    identity-present / membership-absent, identical on the surface to
//!    evicted residue. No marker is ever written for a join, so the sweep
//!    leaves it alone.
//! 2. **Non-TEE soft-leave** — a regular member kicked from a namespace ends
//!    up identity-present / membership-absent too, because the role row is
//!    erased at removal. The soft-leave invariant REQUIRES keeping those
//!    local rows for kick-and-rejoin-keyshare / inheritance-rejoin. No
//!    marker is written for a non-TEE removal (the listener gates on
//!    `TeeMemberRemoved`), so the sweep leaves it alone.
//!
//! # Lifecycle (driven by `calimero-context`'s `self_purge`)
//!
//! * **mark** — written at dispatch time, BEFORE the cascade runs, only when
//!   the removal was confirmed (via `decide_purge_action`) to be a
//!   `TeeMemberRemoved` targeting this node's identity at the namespace
//!   root. Writing before the cascade also covers a crash mid-cascade.
//! * **clear** — written once the cascade FULLY completes (signing keys
//!   gone). On a signing-key purge failure the marker is LEFT so the
//!   reconcile retries on the next restart.
//! * **is_marked** / **iter_pending** — read by the startup reconcile sweep
//!   to enumerate and gate the namespaces it may complete.
//!
//! Pure store rows; presence of the key IS the marker (value is `()`),
//! mirroring [`crate::DenyListRepository`].

use calimero_context_config::types::ContextGroupId;
use calimero_store::key::{PendingSelfPurge, PENDING_SELF_PURGE_PREFIX};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::collect_keys_with_prefix;

/// Typed Repository for the durable pending-self-purge marker set.
///
/// Thin layer over `&Store`. The "marked implies confirmed TEE
/// self-eviction" invariant is the caller's responsibility — the
/// self-purge listener only marks after `decide_purge_action` returns
/// `PurgeAction::Namespace` for a `TeeMemberRemoved` event.
pub struct PendingSelfPurgeRepository<'a> {
    store: &'a Store,
}

impl<'a> PendingSelfPurgeRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Mark `namespace_id` as pending self-purge. Idempotent — a RocksDB put
    /// on an existing key overwrites the same `()` marker.
    ///
    /// **Caller contract:** invoke only after confirming (via the self-purge
    /// dispatch decision) that this node was TEE-self-evicted from the
    /// namespace root, and BEFORE running the local-state cascade so a crash
    /// mid-cascade leaves a retry anchor.
    pub fn mark(&self, namespace_id: &ContextGroupId) -> EyreResult<()> {
        let key = PendingSelfPurge::new(namespace_id.to_bytes());
        let mut handle = self.store.handle();
        handle
            .put(&key, &())
            .map_err(|e| eyre::eyre!("PendingSelfPurgeRepository::mark: {e}"))?;
        Ok(())
    }

    /// Clear `namespace_id`'s marker. Idempotent — clearing an absent marker
    /// is a no-op. Invoked once the cascade fully completes (signing keys
    /// gone) or when the sweep finds the marker stale (already purged, or
    /// re-admitted as a live member).
    pub fn clear(&self, namespace_id: &ContextGroupId) -> EyreResult<()> {
        let key = PendingSelfPurge::new(namespace_id.to_bytes());
        let mut handle = self.store.handle();
        handle
            .delete(&key)
            .map_err(|e| eyre::eyre!("PendingSelfPurgeRepository::clear: {e}"))?;
        Ok(())
    }

    /// Whether `namespace_id` currently carries a pending-self-purge marker.
    /// O(1) key lookup.
    pub fn is_marked(&self, namespace_id: &ContextGroupId) -> EyreResult<bool> {
        let key = PendingSelfPurge::new(namespace_id.to_bytes());
        let handle = self.store.handle();
        handle
            .has(&key)
            .map_err(|e| eyre::eyre!("PendingSelfPurgeRepository::is_marked: {e}"))
    }

    /// Enumerate every marked namespace. Range-scans the
    /// `PendingSelfPurge` column family by prefix — the same seek-and-walk
    /// convention `collect_keys_with_prefix` uses everywhere else in this
    /// crate. The startup reconcile sweep walks this to find namespaces it
    /// may complete (#2721).
    pub fn iter_pending(&self) -> EyreResult<Vec<ContextGroupId>> {
        let keys = collect_keys_with_prefix(
            self.store,
            PendingSelfPurge::new([0u8; 32]),
            PENDING_SELF_PURGE_PREFIX,
            |_k| true,
        )?;
        Ok(keys
            .into_iter()
            .map(|k| ContextGroupId::from(k.namespace_id()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{test_group_id, test_store};

    #[test]
    fn is_marked_returns_false_when_unset() {
        let store = test_store();
        let repo = PendingSelfPurgeRepository::new(&store);
        assert!(!repo.is_marked(&test_group_id()).unwrap());
    }

    #[test]
    fn mark_then_is_marked_round_trip() {
        let store = test_store();
        let repo = PendingSelfPurgeRepository::new(&store);
        let ns = test_group_id();

        repo.mark(&ns).unwrap();
        assert!(repo.is_marked(&ns).unwrap());
    }

    #[test]
    fn clear_after_mark_returns_to_unmarked() {
        let store = test_store();
        let repo = PendingSelfPurgeRepository::new(&store);
        let ns = test_group_id();

        repo.mark(&ns).unwrap();
        repo.clear(&ns).unwrap();
        assert!(!repo.is_marked(&ns).unwrap());
    }

    #[test]
    fn clear_is_idempotent_when_unset() {
        let store = test_store();
        let repo = PendingSelfPurgeRepository::new(&store);
        // Clearing an absent marker must succeed silently.
        repo.clear(&test_group_id()).unwrap();
    }

    #[test]
    fn mark_is_idempotent_on_already_marked() {
        let store = test_store();
        let repo = PendingSelfPurgeRepository::new(&store);
        let ns = test_group_id();

        repo.mark(&ns).unwrap();
        repo.mark(&ns).unwrap();
        assert!(repo.is_marked(&ns).unwrap());
    }

    #[test]
    fn iter_pending_returns_empty_on_fresh_store() {
        let store = test_store();
        let repo = PendingSelfPurgeRepository::new(&store);
        assert!(repo.iter_pending().unwrap().is_empty());
    }

    #[test]
    fn iter_pending_returns_all_marked_namespaces() {
        let store = test_store();
        let repo = PendingSelfPurgeRepository::new(&store);
        let ns_a = ContextGroupId::from([0x11; 32]);
        let ns_b = ContextGroupId::from([0x22; 32]);
        let ns_c = ContextGroupId::from([0x33; 32]);
        repo.mark(&ns_a).unwrap();
        repo.mark(&ns_b).unwrap();
        repo.mark(&ns_c).unwrap();

        let mut got = repo.iter_pending().unwrap();
        got.sort_by_key(|g| g.to_bytes());
        assert_eq!(got, vec![ns_a, ns_b, ns_c]);
    }

    #[test]
    fn iter_pending_excludes_adjacent_column_families() {
        // The scan must stop at the PendingSelfPurge prefix boundary and not
        // bleed into neighbouring column families (e.g. NamespaceIdentity /
        // GroupDeniedMember rows written under different prefixes).
        use calimero_primitives::identity::PublicKey;
        use calimero_store::key::{GroupDeniedMember, NamespaceIdentity, NamespaceIdentityValue};

        let store = test_store();
        let repo = PendingSelfPurgeRepository::new(&store);
        let ns = ContextGroupId::from([0x44; 32]);
        repo.mark(&ns).unwrap();

        // Seed unrelated rows in adjacent prefixes.
        {
            let mut handle = store.handle();
            handle
                .put(
                    &NamespaceIdentity::new([0x44; 32]),
                    &NamespaceIdentityValue {
                        public_key: [0x01; 32],
                        private_key: [0x02; 32],
                        sender_key: [0x03; 32],
                    },
                )
                .unwrap();
            handle
                .put(
                    &GroupDeniedMember::new([0x44; 32], PublicKey::from([0x05; 32])),
                    &(),
                )
                .unwrap();
        }

        let got = repo.iter_pending().unwrap();
        assert_eq!(
            got,
            vec![ns],
            "only the single PendingSelfPurge marker must be returned, got {got:?}"
        );
    }
}
