use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{ContextMeta, GroupMeta, GroupMetaValue, GROUP_META_PREFIX};
use calimero_store::Store;
use eyre::{eyre, Result as EyreResult};
use sha2::{Digest, Sha256};

use super::{collect_keys_with_prefix_paginated, enumerate_group_contexts, list_group_members};

/// Typed Repository for `GroupMetaValue` rows + the derived
/// state-hash computation that gates SignedGroupOp apply.
///
/// Holds the consensus-relevant identity for each group
/// (admin/owner, target application, upgrade policy). Excludes the
/// freeform metadata (`name` / `data`); that's on
/// [`MetadataRepository`].
///
/// Issue #2303 / epic #2300.
pub struct MetaRepository<'a> {
    store: &'a Store,
}

impl<'a> MetaRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn load(&self, group_id: &ContextGroupId) -> EyreResult<Option<GroupMetaValue>> {
        let handle = self.store.handle();
        let key = GroupMeta::new(group_id.to_bytes());
        Ok(handle.get(&key)?)
    }

    pub fn save(&self, group_id: &ContextGroupId, meta: &GroupMetaValue) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = GroupMeta::new(group_id.to_bytes());
        handle.put(&key, meta)?;
        Ok(())
    }

    pub fn delete(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = GroupMeta::new(group_id.to_bytes());
        handle.delete(&key)?;
        Ok(())
    }

    pub fn enumerate_all(
        &self,
        offset: usize,
        limit: usize,
    ) -> EyreResult<Vec<([u8; 32], GroupMetaValue)>> {
        let keys = collect_keys_with_prefix_paginated(
            self.store,
            GroupMeta::new([0u8; 32]),
            GROUP_META_PREFIX,
            |_| true,
            offset,
            limit,
        )?;
        let handle = self.store.handle();
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
    ///
    /// Note: metadata records (`name` / `data` / `updated_at` / `updated_by`) are
    /// intentionally **excluded** from this hash — exactly as the former alias rows
    /// were — so the hash stays a function of consensus-relevant state only.
    pub fn compute_state_hash(&self, group_id: &ContextGroupId) -> EyreResult<[u8; 32]> {
        let meta = self
            .load(group_id)?
            .ok_or_else(|| eyre!("group not found for state hash computation"))?;

        let mut members = list_group_members(self.store, group_id, 0, usize::MAX)?;
        members.sort_by(|a, b| a.0.cmp(&b.0));
        // Defensive dedup against the theoretical case of duplicate
        // `GroupMember` rows (store corruption only).
        members.dedup_by(|a, b| a.0 == b.0);

        hash_group_state(group_id, &meta, &members)
    }

    /// Return the group state hash that would result if `removed_member`
    /// were dropped from the group's member set. Pure simulation: reads
    /// the current materialized state, removes the named identity from
    /// the sorted-by-pubkey member list in-memory, and hashes.
    ///
    /// Used at sign time so the admin (or leaver) can populate the
    /// `expected_group_state_hash` field on `MemberRemoved` /
    /// `MemberLeft` before the apply runs locally.
    pub fn compute_state_hash_after_remove(
        &self,
        group_id: &ContextGroupId,
        removed_member: &PublicKey,
    ) -> EyreResult<[u8; 32]> {
        let meta = self
            .load(group_id)?
            .ok_or_else(|| eyre!("group not found for state hash computation"))?;

        let mut members = list_group_members(self.store, group_id, 0, usize::MAX)?;
        members.retain(|(pk, _role)| pk != removed_member);
        members.sort_by(|a, b| a.0.cmp(&b.0));
        members.dedup_by(|a, b| a.0 == b.0);

        hash_group_state(group_id, &meta, &members)
    }

    /// Snapshot the current CRDT root hash for every context registered
    /// under `group_id`. Returned sorted by `context_id` for
    /// deterministic op-content hashing.
    ///
    /// Contexts whose `ContextMeta` row is missing (registered in the
    /// group index but not yet materialized) are skipped, not errored
    /// — see the asymmetric-skip rationale in the original module doc.
    pub fn snapshot_context_state_hashes(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<Vec<(ContextId, [u8; 32])>> {
        let context_ids = enumerate_group_contexts(self.store, group_id, 0, usize::MAX)?;
        let handle = self.store.handle();
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
}

/// Single source of truth for the group state hash byte layout.
///
/// **Caller contract**: `members` MUST be sorted by `PublicKey` byte
/// ordering. The hash is order-sensitive; an unsorted slice produces a
/// different digest for the same logical set and breaks convergence.
fn hash_group_state(
    group_id: &ContextGroupId,
    meta: &GroupMetaValue,
    members_sorted: &[(PublicKey, GroupMemberRole)],
) -> EyreResult<[u8; 32]> {
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

// ---------------------------------------------------------------------------
// Deprecated free-function wrappers.
// ---------------------------------------------------------------------------

#[deprecated(note = "use MetaRepository::new(store).load(...)")]
pub fn load_group_meta(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<GroupMetaValue>> {
    MetaRepository::new(store).load(group_id)
}

#[deprecated(note = "use MetaRepository::new(store).save(...)")]
pub fn save_group_meta(
    store: &Store,
    group_id: &ContextGroupId,
    meta: &GroupMetaValue,
) -> EyreResult<()> {
    MetaRepository::new(store).save(group_id, meta)
}

#[deprecated(note = "use MetaRepository::new(store).delete(...)")]
pub fn delete_group_meta(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    MetaRepository::new(store).delete(group_id)
}

#[deprecated(note = "use MetaRepository::new(store).enumerate_all(...)")]
pub fn enumerate_all_groups(
    store: &Store,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<([u8; 32], GroupMetaValue)>> {
    MetaRepository::new(store).enumerate_all(offset, limit)
}

#[deprecated(note = "use MetaRepository::new(store).compute_state_hash(...)")]
pub fn compute_group_state_hash(store: &Store, group_id: &ContextGroupId) -> EyreResult<[u8; 32]> {
    MetaRepository::new(store).compute_state_hash(group_id)
}

#[deprecated(note = "use MetaRepository::new(store).compute_state_hash_after_remove(...)")]
pub fn compute_group_state_hash_after_remove(
    store: &Store,
    group_id: &ContextGroupId,
    removed_member: &PublicKey,
) -> EyreResult<[u8; 32]> {
    MetaRepository::new(store).compute_state_hash_after_remove(group_id, removed_member)
}

#[deprecated(note = "use MetaRepository::new(store).snapshot_context_state_hashes(...)")]
pub fn snapshot_context_state_hashes(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(ContextId, [u8; 32])>> {
    MetaRepository::new(store).snapshot_context_state_hashes(group_id)
}
