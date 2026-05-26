use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_primitives::metadata::MetadataRecord;
use calimero_store::key::{
    GroupContextIndex, GroupContextMetadata, GroupMemberMetadata, GroupMetaValue, GroupMetadata,
    GROUP_CONTEXT_INDEX_PREFIX, GROUP_MEMBER_METADATA_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::{
    check_group_membership, collect_keys_with_prefix, count_group_members, count_keys_with_prefix,
    enumerate_group_contexts, get_parent_group, list_child_groups,
};

/// Typed Repository for freeform metadata records on groups,
/// contexts-within-groups, and members. Separate from
/// [`MetaRepository`] (`GroupMetaValue`-only) because metadata is
/// **not** consensus-relevant: a `MetadataSet` op doesn't change the
/// group state hash, and `updated_at` is applier-stamped.
///
/// Issue #2303 / epic #2300.
pub struct MetadataRepository<'a> {
    store: &'a Store,
}

impl<'a> MetadataRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    // --- Context metadata ---

    pub fn set_context(
        &self,
        group_id: &ContextGroupId,
        context_id: &ContextId,
        record: &MetadataRecord,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.put(
            &GroupContextMetadata::new(group_id.to_bytes(), *context_id),
            record,
        )?;
        Ok(())
    }

    pub fn context_metadata(
        &self,
        group_id: &ContextGroupId,
        context_id: &ContextId,
    ) -> EyreResult<Option<MetadataRecord>> {
        let handle = self.store.handle();
        handle
            .get(&GroupContextMetadata::new(group_id.to_bytes(), *context_id))
            .map_err(Into::into)
    }

    pub fn delete_context(
        &self,
        group_id: &ContextGroupId,
        context_id: &ContextId,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.delete(&GroupContextMetadata::new(group_id.to_bytes(), *context_id))?;
        Ok(())
    }

    pub fn enumerate_contexts_with_names(
        &self,
        group_id: &ContextGroupId,
        offset: usize,
        limit: usize,
    ) -> EyreResult<Vec<(ContextId, Option<String>)>> {
        let ids = enumerate_group_contexts(self.store, group_id, offset, limit)?;
        ids.into_iter()
            .map(|ctx_id| {
                let name = self.context_metadata(group_id, &ctx_id)?.and_then(|r| r.name);
                Ok((ctx_id, name))
            })
            .collect()
    }

    pub fn count_contexts(&self, group_id: &ContextGroupId) -> EyreResult<usize> {
        let gid = group_id.to_bytes();
        count_keys_with_prefix(
            self.store,
            GroupContextIndex::new(gid, ContextId::from([0u8; 32])),
            GROUP_CONTEXT_INDEX_PREFIX,
            |k| k.group_id() == gid,
        )
    }

    // --- Member metadata ---

    pub fn set_member(
        &self,
        group_id: &ContextGroupId,
        member: &PublicKey,
        record: &MetadataRecord,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.put(
            &GroupMemberMetadata::new(group_id.to_bytes(), *member),
            record,
        )?;
        Ok(())
    }

    pub fn member_metadata(
        &self,
        group_id: &ContextGroupId,
        member: &PublicKey,
    ) -> EyreResult<Option<MetadataRecord>> {
        let handle = self.store.handle();
        handle
            .get(&GroupMemberMetadata::new(group_id.to_bytes(), *member))
            .map_err(Into::into)
    }

    pub fn delete_member(
        &self,
        group_id: &ContextGroupId,
        member: &PublicKey,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.delete(&GroupMemberMetadata::new(group_id.to_bytes(), *member))?;
        Ok(())
    }

    pub fn enumerate_members(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<Vec<(PublicKey, MetadataRecord)>> {
        let gid = group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupMemberMetadata::new(gid, PublicKey::from([0u8; 32])),
            GROUP_MEMBER_METADATA_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let handle = self.store.handle();
        let mut results = Vec::new();
        for key in keys {
            let Some(record) = handle.get(&key)? else {
                continue;
            };
            results.push((key.member(), record));
        }
        Ok(results)
    }

    pub fn delete_all_members(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let gid = group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupMemberMetadata::new(gid, PublicKey::from([0u8; 32])),
            GROUP_MEMBER_METADATA_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let mut handle = self.store.handle();
        for key in keys {
            handle.delete(&key)?;
        }
        Ok(())
    }

    // --- Group metadata ---

    pub fn set_group(
        &self,
        group_id: &ContextGroupId,
        record: &MetadataRecord,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.put(&GroupMetadata::new(group_id.to_bytes()), record)?;
        Ok(())
    }

    pub fn group_metadata(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<Option<MetadataRecord>> {
        let handle = self.store.handle();
        handle
            .get(&GroupMetadata::new(group_id.to_bytes()))
            .map_err(Into::into)
    }

    pub fn delete_group(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.delete(&GroupMetadata::new(group_id.to_bytes()))?;
        Ok(())
    }

    /// Build a `NamespaceSummary` for a root group, fetching counts from
    /// the store. Returns `None` if the group has a parent (not a
    /// namespace root) or if `node_identity` is not a member.
    pub fn build_namespace_summary(
        &self,
        group_id: &ContextGroupId,
        meta: &GroupMetaValue,
        node_identity: &PublicKey,
    ) -> EyreResult<Option<calimero_context_client::group::NamespaceSummary>> {
        if get_parent_group(self.store, group_id)?.is_some() {
            return Ok(None);
        }
        if !check_group_membership(self.store, group_id, node_identity)? {
            return Ok(None);
        }

        let name = self
            .group_metadata(group_id)
            .ok()
            .flatten()
            .and_then(|r| r.name);
        let member_count = count_group_members(self.store, group_id).unwrap_or(0);
        let context_count = enumerate_group_contexts(self.store, group_id, 0, usize::MAX)
            .unwrap_or_default()
            .len();
        let subgroup_count = list_child_groups(self.store, group_id)
            .unwrap_or_default()
            .len();

        Ok(Some(calimero_context_client::group::NamespaceSummary {
            namespace_id: *group_id,
            app_key: meta.app_key.into(),
            target_application_id: meta.target_application_id,
            upgrade_policy: meta.upgrade_policy.clone(),
            created_at: meta.created_at,
            name,
            member_count,
            context_count,
            subgroup_count,
        }))
    }
}

// ---------------------------------------------------------------------------
// Deprecated free-function wrappers.
// ---------------------------------------------------------------------------

#[deprecated(note = "use MetadataRepository::new(store).set_context(...)")]
pub fn set_context_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    record: &MetadataRecord,
) -> EyreResult<()> {
    MetadataRepository::new(store).set_context(group_id, context_id, record)
}

#[deprecated(note = "use MetadataRepository::new(store).context_metadata(...)")]
pub fn get_context_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Option<MetadataRecord>> {
    MetadataRepository::new(store).context_metadata(group_id, context_id)
}

#[deprecated(note = "use MetadataRepository::new(store).enumerate_contexts_with_names(...)")]
pub fn enumerate_group_contexts_with_names(
    store: &Store,
    group_id: &ContextGroupId,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<(ContextId, Option<String>)>> {
    MetadataRepository::new(store).enumerate_contexts_with_names(group_id, offset, limit)
}

#[deprecated(note = "use MetadataRepository::new(store).set_member(...)")]
pub fn set_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    record: &MetadataRecord,
) -> EyreResult<()> {
    MetadataRepository::new(store).set_member(group_id, member, record)
}

#[deprecated(note = "use MetadataRepository::new(store).member_metadata(...)")]
pub fn get_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Option<MetadataRecord>> {
    MetadataRepository::new(store).member_metadata(group_id, member)
}

#[deprecated(note = "use MetadataRepository::new(store).set_group(...)")]
pub fn set_group_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    record: &MetadataRecord,
) -> EyreResult<()> {
    MetadataRepository::new(store).set_group(group_id, record)
}

#[deprecated(note = "use MetadataRepository::new(store).group_metadata(...)")]
pub fn get_group_metadata(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<MetadataRecord>> {
    MetadataRepository::new(store).group_metadata(group_id)
}

#[deprecated(note = "use MetadataRepository::new(store).build_namespace_summary(...)")]
pub fn build_namespace_summary(
    store: &Store,
    group_id: &ContextGroupId,
    meta: &GroupMetaValue,
    node_identity: &PublicKey,
) -> EyreResult<Option<calimero_context_client::group::NamespaceSummary>> {
    MetadataRepository::new(store).build_namespace_summary(group_id, meta, node_identity)
}

#[deprecated(note = "use MetadataRepository::new(store).enumerate_members(...)")]
pub fn enumerate_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(PublicKey, MetadataRecord)>> {
    MetadataRepository::new(store).enumerate_members(group_id)
}

#[deprecated(note = "use MetadataRepository::new(store).count_contexts(...)")]
pub fn count_group_contexts(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    MetadataRepository::new(store).count_contexts(group_id)
}

#[deprecated(note = "use MetadataRepository::new(store).delete_group(...)")]
pub fn delete_group_metadata(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    MetadataRepository::new(store).delete_group(group_id)
}

#[deprecated(note = "use MetadataRepository::new(store).delete_member(...)")]
pub fn delete_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    MetadataRepository::new(store).delete_member(group_id, member)
}

#[deprecated(note = "use MetadataRepository::new(store).delete_context(...)")]
pub fn delete_context_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    MetadataRepository::new(store).delete_context(group_id, context_id)
}

#[deprecated(note = "use MetadataRepository::new(store).delete_all_members(...)")]
pub fn delete_all_member_metadata(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    MetadataRepository::new(store).delete_all_members(group_id)
}
