use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    ContextGroupRef, ContextIdentity, GroupContextIndex, GROUP_CONTEXT_INDEX_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::collect_keys_with_prefix;

/// Service for context/group index traversal and mutations.
pub struct ContextTreeService<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
}

impl<'a> ContextTreeService<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        Self { store, group_id }
    }

    pub fn register_context(&self, context_id: &ContextId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let group_id_bytes = self.group_id.to_bytes();

        // If already registered in a different group, remove the stale index entry.
        let ref_key = ContextGroupRef::new(*context_id);
        if let Some(existing_group_bytes) = handle.get(&ref_key)? {
            if existing_group_bytes != group_id_bytes {
                let old_idx = GroupContextIndex::new(existing_group_bytes, *context_id);
                handle.delete(&old_idx)?;
            }
        }

        let idx_key = GroupContextIndex::new(group_id_bytes, *context_id);
        handle.put(&idx_key, &())?;
        handle.put(&ref_key, &group_id_bytes)?;

        Ok(())
    }

    pub fn unregister_context(&self, context_id: &ContextId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let group_id_bytes = self.group_id.to_bytes();

        let idx_key = GroupContextIndex::new(group_id_bytes, *context_id);
        handle.delete(&idx_key)?;

        let ref_key = ContextGroupRef::new(*context_id);
        handle.delete(&ref_key)?;

        Ok(())
    }

    /// Alias for `unregister_context` for detach-style call sites.
    pub fn detach_context(&self, context_id: &ContextId) -> EyreResult<()> {
        self.unregister_context(context_id)
    }

    pub fn group_for_context(&self, context_id: &ContextId) -> EyreResult<Option<ContextGroupId>> {
        let handle = self.store.handle();
        let key = ContextGroupRef::new(*context_id);
        let value = handle.get(&key)?;
        Ok(value.map(ContextGroupId::from))
    }

    pub fn enumerate_contexts(&self, offset: usize, limit: usize) -> EyreResult<Vec<ContextId>> {
        let gid = self.group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupContextIndex::new(gid, ContextId::from([0u8; 32])),
            GROUP_CONTEXT_INDEX_PREFIX,
            |k| k.group_id() == gid,
        )?;
        Ok(keys
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|k| k.context_id())
            .collect())
    }

    /// Backward-compatible alias for `enumerate_contexts`.
    pub fn list_contexts(&self, offset: usize, limit: usize) -> EyreResult<Vec<ContextId>> {
        self.enumerate_contexts(offset, limit)
    }

    pub fn cascade_remove_member(&self, member: &PublicKey) -> EyreResult<()> {
        let contexts = self.enumerate_contexts(0, usize::MAX)?;
        let mut handle = self.store.handle();
        for context_id in &contexts {
            let identity_key = ContextIdentity::new(*context_id, (*member).into());
            if handle.has(&identity_key)? {
                handle.delete(&identity_key)?;
                tracing::info!(
                    group_id = %hex::encode(self.group_id.to_bytes()),
                    context_id = %hex::encode(context_id.as_ref()),
                    member = %member,
                    "cascade-removed member from context"
                );
            }
        }

        Ok(())
    }

    /// Scans ContextIdentity rows for this context and returns first identity
    /// that has a locally available private key.
    pub fn find_local_signing_identity(
        &self,
        context_id: &ContextId,
    ) -> EyreResult<Option<PublicKey>> {
        let handle = self.store.handle();
        let start_key = ContextIdentity::new(*context_id, [0u8; 32].into());
        let mut iter = handle.iter::<ContextIdentity>()?;
        let first = iter.seek(start_key).transpose();

        for key_result in first.into_iter().chain(iter.keys()) {
            let key = key_result?;
            if key.context_id() != *context_id {
                break;
            }
            let Some(value) = handle.get(&key)? else {
                continue;
            };
            if value.private_key.is_some() {
                return Ok(Some(key.public_key()));
            }
        }

        Ok(None)
    }
}
