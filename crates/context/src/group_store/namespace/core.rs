use crate::group_store::{MembershipRepository, MetaRepository};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    GroupChildIndex, GroupParentRef, NamespaceIdentity, NamespaceIdentityValue,
    GROUP_CHILD_INDEX_PREFIX,
};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use rand::rngs::OsRng;
use rand::Rng;
use sha2::Digest;

use super::super::{
    cascade_remove_member_from_group_tree, collect_keys_with_prefix, get_group_for_context,
};

pub(crate) const MAX_NAMESPACE_DEPTH: usize = 16;

#[derive(Debug, Clone, Copy)]
pub struct NamespaceIdentityRecord {
    pub public_key: PublicKey,
    pub private_key: [u8; 32],
    pub sender_key: [u8; 32],
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedNamespaceIdentity {
    pub namespace_id: ContextGroupId,
    pub identity: NamespaceIdentityRecord,
}

/// Result of subtree enumeration. `descendant_groups` does NOT include the
/// root itself. Order is children-first (deepest descendants come first),
/// matching the order required by `execute_group_deleted` for safe child-index
/// cleanup.
#[derive(Debug, Clone)]
pub struct CascadePayload {
    pub descendant_groups: Vec<ContextGroupId>,
    pub contexts: Vec<ContextId>,
}

/// Outcome of a `reparent_group` call. Distinguishes the no-op idempotent
/// case from an actual edge swap so callers can report accurately and
/// suppress misleading "reparented" events when nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReparentOutcome {
    /// Edges were swapped; the structural shape of the tree changed.
    Reparented {
        /// The parent before the swap (now no longer a parent of child).
        old_parent: ContextGroupId,
    },
    /// `new_parent == old_parent` — no writes performed, no shape change.
    Unchanged,
}

/// Typed Repository for namespace topology, identity, and tree-walk
/// operations. Sibling to the service-style Repositories already in
/// the namespace cluster (`NamespaceGovernance`, `NamespaceDagService`,
/// `NamespaceOpLogService`, `NamespaceRetryService`,
/// `NamespaceMembershipService`) — covers the topology half:
/// parent/child edges, descendant walks, reparent, identity records.
///
/// Issue #2303 / epic #2300.
pub struct NamespaceRepository<'a> {
    store: &'a Store,
}

impl<'a> NamespaceRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Returns `true` if the member has a read-only role (`ReadOnly` or
    /// `ReadOnlyTee`) in the group that owns this context.
    pub fn is_read_only_for_context(
        &self,
        context_id: &ContextId,
        identity: &PublicKey,
    ) -> EyreResult<bool> {
        let Some(group_id) = get_group_for_context(self.store, context_id)? else {
            return Ok(false);
        };
        match MembershipRepository::new(self.store).role_of(&group_id, identity)? {
            Some(
                calimero_primitives::context::GroupMemberRole::ReadOnly
                | calimero_primitives::context::GroupMemberRole::ReadOnlyTee,
            ) => Ok(true),
            _ => Ok(false),
        }
    }

    /// Returns `true` if `executor` is currently authorized to author state
    /// mutations on `context_id` — direct admin/member or Open-subgroup
    /// inheritance. See original `is_authorized_for_context_state_op` doc
    /// for full semantics.
    pub fn is_authorized_for_context_state_op(
        &self,
        context_id: &ContextId,
        executor: &PublicKey,
    ) -> EyreResult<bool> {
        let Some(group_id) = get_group_for_context(self.store, context_id)? else {
            return Ok(true);
        };

        if MembershipRepository::new(self.store).is_admin(&group_id, executor)? {
            return Ok(true);
        }

        if let Some(role) = MembershipRepository::new(self.store).role_of(&group_id, executor)? {
            return Ok(matches!(
                role,
                calimero_primitives::context::GroupMemberRole::Admin
                    | calimero_primitives::context::GroupMemberRole::Member,
            ));
        }

        match MembershipRepository::new(self.store).check_path(&group_id, executor)? {
            super::super::membership::MembershipPath::Direct => Ok(true),
            super::super::membership::MembershipPath::Inherited { .. } => Ok(true),
            super::super::membership::MembershipPath::None => Ok(false),
        }
    }

    pub fn parent(&self, group_id: &ContextGroupId) -> EyreResult<Option<ContextGroupId>> {
        let handle = self.store.handle();
        let key = GroupParentRef::new(group_id.to_bytes());
        Ok(handle.get(&key)?.map(ContextGroupId::from))
    }

    /// **Test/legacy helper.** Direct store write of a parent edge.
    /// Production code MUST emit `RootOp::GroupCreated` or `GroupReparented`.
    #[doc(hidden)]
    pub fn nest(
        &self,
        parent_group_id: &ContextGroupId,
        child_group_id: &ContextGroupId,
    ) -> EyreResult<()> {
        if parent_group_id == child_group_id {
            bail!("cannot nest a group under itself");
        }

        if self.parent(child_group_id)?.is_some() {
            bail!(
                "group {:?} already has a parent; unnest it first",
                child_group_id
            );
        }

        let mut current = *parent_group_id;
        let mut depth = 0usize;
        while let Some(ancestor) = self.parent(&current)? {
            if ancestor == *child_group_id {
                bail!("nesting would create a cycle");
            }
            depth += 1;
            if depth > MAX_NAMESPACE_DEPTH {
                bail!(
                    "nesting depth exceeds MAX_NAMESPACE_DEPTH ({MAX_NAMESPACE_DEPTH}); \
                     a tree this deep would also be unwalkable by every other \
                     parent-chain operation (resolve, check_path, is_descendant_of)"
                );
            }
            current = ancestor;
        }

        let mut handle = self.store.handle();
        let ref_key = GroupParentRef::new(child_group_id.to_bytes());
        handle.put(&ref_key, &parent_group_id.to_bytes())?;
        let idx_key = GroupChildIndex::new(parent_group_id.to_bytes(), child_group_id.to_bytes());
        handle.put(&idx_key, &())?;
        Ok(())
    }

    /// **Test/legacy helper.** Direct store delete of a parent edge.
    /// Production code MUST emit `RootOp::GroupReparented` or `GroupDeleted`.
    #[doc(hidden)]
    pub fn unnest(
        &self,
        parent_group_id: &ContextGroupId,
        child_group_id: &ContextGroupId,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let ref_key = GroupParentRef::new(child_group_id.to_bytes());
        handle.delete(&ref_key)?;
        let idx_key = GroupChildIndex::new(parent_group_id.to_bytes(), child_group_id.to_bytes());
        handle.delete(&idx_key)?;
        Ok(())
    }

    /// List all direct children of a group.
    pub fn list_children(
        &self,
        parent_group_id: &ContextGroupId,
    ) -> EyreResult<Vec<ContextGroupId>> {
        let pid = parent_group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupChildIndex::new(pid, [0u8; 32]),
            GROUP_CHILD_INDEX_PREFIX,
            |k| k.parent_group_id() == pid,
        )?;
        Ok(keys
            .into_iter()
            .map(|k| ContextGroupId::from(k.child_group_id()))
            .collect())
    }

    /// Collect ALL descendant group IDs by walking the child index
    /// (iterative DFS via explicit stack), excluding the starting group.
    pub fn collect_descendants(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<Vec<ContextGroupId>> {
        let mut descendants = Vec::new();
        let mut stack = vec![*group_id];

        while let Some(current) = stack.pop() {
            let children = self.list_children(&current)?;
            for child in children {
                descendants.push(child);
                stack.push(child);
            }
        }

        Ok(descendants)
    }

    /// Collect descendant group IDs **visible to `viewer`**. See original
    /// `collect_visible_descendant_groups` doc for full visibility rules.
    pub fn collect_visible_descendants(
        &self,
        group_id: &ContextGroupId,
        viewer: &PublicKey,
    ) -> EyreResult<Vec<ContextGroupId>> {
        let mut descendants = Vec::new();
        let mut stack = vec![*group_id];

        while let Some(current) = stack.pop() {
            for child in self.list_children(&current)? {
                if !MembershipRepository::new(self.store).is_member(&child, viewer)? {
                    continue;
                }
                descendants.push(child);
                stack.push(child);
            }
        }

        Ok(descendants)
    }

    /// Create invitations for a group AND all of its descendant groups
    /// that are visible to the inviter. Returns
    /// `(group_id, SignedGroupOpenInvitation)` pairs.
    pub fn create_recursive_invitations(
        &self,
        root_group_id: &ContextGroupId,
        inviter_sk: &PrivateKey,
        expiration_secs: u64,
        invited_role: u8,
    ) -> EyreResult<
        Vec<(
            ContextGroupId,
            calimero_context_config::types::SignedGroupOpenInvitation,
        )>,
    > {
        use calimero_context_config::types::{
            GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
        };

        let mut groups = vec![*root_group_id];
        groups.extend(self.collect_visible_descendants(root_group_id, &inviter_sk.public_key())?);

        let inviter_signer_id = SignerId::from(*inviter_sk.public_key());
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expiration = now_secs + expiration_secs;

        let mut result = Vec::with_capacity(groups.len());
        for gid in groups {
            let secret_salt: [u8; 32] = OsRng.gen();

            let invitation = GroupInvitationFromAdmin {
                inviter_identity: inviter_signer_id,
                group_id: gid,
                expiration_timestamp: expiration,
                secret_salt,
                invited_role,
            };

            let inv_bytes = borsh::to_vec(&invitation).map_err(|e| eyre::eyre!("borsh: {e}"))?;
            let hash = sha2::Sha256::digest(&inv_bytes);
            let sig = inviter_sk
                .sign(&hash)
                .map_err(|e| eyre::eyre!("signing: {e}"))?;

            let application_id = match MetaRepository::new(self.store).load(&gid)? {
                Some(meta) => Some(*meta.target_application_id.as_ref()),
                None => {
                    tracing::warn!(
                        group_id = %hex::encode(gid.to_bytes()),
                        "create_recursive_invitations: missing GroupMeta for descendant; \
                         issuing invitation with application_id = None (joiner will fall back to zero)"
                    );
                    None
                }
            };

            let signed = SignedGroupOpenInvitation {
                invitation,
                inviter_signature: hex::encode(sig.to_bytes()),
                application_id,
            };

            result.push((gid, signed));
        }

        Ok(result)
    }

    /// Remove a member from a group AND all its descendant groups
    /// (direct memberships only). Returns the groups they were
    /// directly removed from.
    pub fn recursive_remove_member(
        &self,
        root_group_id: &ContextGroupId,
        member: &PublicKey,
    ) -> EyreResult<Vec<ContextGroupId>> {
        let mut groups = vec![*root_group_id];
        groups.extend(self.collect_descendants(root_group_id)?);

        let mut removed_from = Vec::new();
        for gid in &groups {
            if MembershipRepository::new(self.store)
                .role_of(gid, member)?
                .is_some()
            {
                MembershipRepository::new(self.store).remove_member(gid, member)?;
                cascade_remove_member_from_group_tree(self.store, gid, member)?;
                removed_from.push(*gid);
            }
        }

        Ok(removed_from)
    }

    /// Walk the parent chain to find the root group (namespace).
    pub fn resolve(&self, group_id: &ContextGroupId) -> EyreResult<ContextGroupId> {
        let mut current = *group_id;
        for _ in 0..MAX_NAMESPACE_DEPTH {
            match self.parent(&current)? {
                Some(parent) => current = parent,
                None => return Ok(current),
            }
        }
        eyre::bail!(
            "namespace resolution exceeded max depth ({MAX_NAMESPACE_DEPTH}), possible circular reference"
        )
    }

    /// Walk the subtree rooted at `root` and return:
    /// - every descendant `group_id` in children-first order
    /// - every `context_id` registered on `root` or any descendant
    pub fn collect_subtree_for_cascade(&self, root: &ContextGroupId) -> EyreResult<CascadePayload> {
        let mut contexts: Vec<ContextId> = Vec::new();
        contexts.extend(super::super::enumerate_group_contexts(
            self.store,
            root,
            0,
            usize::MAX,
        )?);

        let mut dfs_preorder: Vec<ContextGroupId> = Vec::new();
        let mut stack = vec![*root];
        while let Some(g) = stack.pop() {
            for child in self.list_children(&g)? {
                dfs_preorder.push(child);
                stack.push(child);
                contexts.extend(super::super::enumerate_group_contexts(
                    self.store,
                    &child,
                    0,
                    usize::MAX,
                )?);
            }
        }
        let descendant_groups = dfs_preorder.into_iter().rev().collect();
        Ok(CascadePayload {
            descendant_groups,
            contexts,
        })
    }

    /// Atomically swap the parent of `child` to `new_parent`. Replaces
    /// the old `nest_group` + `unnest_group` two-step pattern.
    pub fn reparent(
        &self,
        child: &ContextGroupId,
        new_parent: &ContextGroupId,
    ) -> EyreResult<ReparentOutcome> {
        let old_parent = self.parent(child)?.ok_or_else(|| {
            eyre::eyre!("cannot reparent the namespace root: '{child:?}' has no parent")
        })?;

        if old_parent == *new_parent {
            return Ok(ReparentOutcome::Unchanged);
        }

        if MetaRepository::new(self.store).load(new_parent)?.is_none() {
            eyre::bail!("new parent group '{new_parent:?}' not found in this namespace");
        }

        if self.is_descendant_of(new_parent, child)? {
            eyre::bail!("cycle: new_parent '{new_parent:?}' is a descendant of child '{child:?}'");
        }

        let mut handle = self.store.handle();
        handle.delete(&GroupChildIndex::new(
            old_parent.to_bytes(),
            child.to_bytes(),
        ))?;
        handle.put(
            &GroupParentRef::new(child.to_bytes()),
            &new_parent.to_bytes(),
        )?;
        handle.put(
            &GroupChildIndex::new(new_parent.to_bytes(), child.to_bytes()),
            &(),
        )?;
        Ok(ReparentOutcome::Reparented { old_parent })
    }

    /// Returns `true` iff `candidate` is a (transitive) descendant of
    /// `potential_ancestor`. Returns `false` for `candidate == potential_ancestor`.
    pub fn is_descendant_of(
        &self,
        candidate: &ContextGroupId,
        potential_ancestor: &ContextGroupId,
    ) -> EyreResult<bool> {
        if candidate == potential_ancestor {
            return Ok(false);
        }
        let mut current = *candidate;
        for _ in 0..MAX_NAMESPACE_DEPTH {
            match self.parent(&current)? {
                Some(parent) => {
                    if parent == *potential_ancestor {
                        return Ok(true);
                    }
                    current = parent;
                }
                None => return Ok(false),
            }
        }
        eyre::bail!(
            "is_descendant_of exceeded MAX_NAMESPACE_DEPTH ({MAX_NAMESPACE_DEPTH}); possible cycle in store"
        )
    }

    /// Read this node's identity for a namespace from the store.
    pub fn identity(
        &self,
        namespace_id: &ContextGroupId,
    ) -> EyreResult<Option<(PublicKey, [u8; 32], [u8; 32])>> {
        Ok(self
            .identity_record(namespace_id)?
            .map(|record| (record.public_key, record.private_key, record.sender_key)))
    }

    pub fn identity_record(
        &self,
        namespace_id: &ContextGroupId,
    ) -> EyreResult<Option<NamespaceIdentityRecord>> {
        let handle = self.store.handle();
        let key = NamespaceIdentity::new(namespace_id.to_bytes());
        match handle.get(&key)? {
            Some(val) => Ok(Some(NamespaceIdentityRecord {
                public_key: PublicKey::from(val.public_key),
                private_key: val.private_key,
                sender_key: val.sender_key,
            })),
            None => Ok(None),
        }
    }

    pub fn store_identity(
        &self,
        namespace_id: &ContextGroupId,
        public_key: &PublicKey,
        private_key: &[u8; 32],
        sender_key: &[u8; 32],
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = NamespaceIdentity::new(namespace_id.to_bytes());
        handle.put(
            &key,
            &NamespaceIdentityValue {
                public_key: **public_key,
                private_key: *private_key,
                sender_key: *sender_key,
            },
        )?;
        Ok(())
    }

    /// Resolve the namespace for a group and return this node's identity.
    pub fn resolve_identity(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<Option<(PublicKey, [u8; 32], [u8; 32])>> {
        Ok(self
            .resolve_identity_record(group_id)?
            .map(|record| (record.public_key, record.private_key, record.sender_key)))
    }

    pub fn resolve_identity_record(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<Option<NamespaceIdentityRecord>> {
        let ns_id = self.resolve(group_id)?;
        self.identity_record(&ns_id)
    }

    /// Resolve the namespace for a group and return this node's identity,
    /// generating and storing a new keypair if none exists.
    pub fn get_or_create_identity(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<(ContextGroupId, PublicKey, [u8; 32], [u8; 32])> {
        let bundle = self.get_or_create_identity_bundle(group_id)?;
        Ok((
            bundle.namespace_id,
            bundle.identity.public_key,
            bundle.identity.private_key,
            bundle.identity.sender_key,
        ))
    }

    pub fn get_or_create_identity_bundle(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<ResolvedNamespaceIdentity> {
        let ns_id = self.resolve(group_id)?;
        if let Some(identity) = self.identity_record(&ns_id)? {
            return Ok(ResolvedNamespaceIdentity {
                namespace_id: ns_id,
                identity,
            });
        }

        let private_key = PrivateKey::random(&mut OsRng);
        let public_key = private_key.public_key();
        let sender_key = PrivateKey::random(&mut OsRng);

        self.store_identity(&ns_id, &public_key, &private_key, &sender_key)?;

        Ok(ResolvedNamespaceIdentity {
            namespace_id: ns_id,
            identity: NamespaceIdentityRecord {
                public_key,
                private_key: *private_key,
                sender_key: *sender_key,
            },
        })
    }
}

/// Repository-API smoke tests. Topology + namespace-feature coverage
/// (recursive remove, visible-descendant walks, cascade, reparent
/// cycle detection, etc.) lives in the cluster-level
/// `namespace/tests.rs`; this module is the thin "Repository surface
/// dispatches correctly" check.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_store::test_fixtures::test_store;

    fn gid(seed: u8) -> ContextGroupId {
        ContextGroupId::from([seed; 32])
    }

    #[test]
    fn parent_returns_none_for_unrooted_group() {
        let store = test_store();
        let repo = NamespaceRepository::new(&store);
        assert!(repo.parent(&gid(1)).unwrap().is_none());
    }

    #[test]
    fn nest_then_parent_round_trip() {
        let store = test_store();
        let repo = NamespaceRepository::new(&store);
        let parent = gid(1);
        let child = gid(2);
        repo.nest(&parent, &child).unwrap();
        assert_eq!(repo.parent(&child).unwrap(), Some(parent));
    }

    #[test]
    fn list_children_after_nest() {
        let store = test_store();
        let repo = NamespaceRepository::new(&store);
        let parent = gid(1);
        repo.nest(&parent, &gid(2)).unwrap();
        repo.nest(&parent, &gid(3)).unwrap();
        let children = repo.list_children(&parent).unwrap();
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn resolve_walks_to_root() {
        let store = test_store();
        let repo = NamespaceRepository::new(&store);
        let root = gid(1);
        let middle = gid(2);
        let leaf = gid(3);
        repo.nest(&root, &middle).unwrap();
        repo.nest(&middle, &leaf).unwrap();
        assert_eq!(repo.resolve(&leaf).unwrap(), root);
        assert_eq!(repo.resolve(&middle).unwrap(), root);
        assert_eq!(repo.resolve(&root).unwrap(), root);
    }

    #[test]
    fn is_descendant_of_recognises_chain() {
        let store = test_store();
        let repo = NamespaceRepository::new(&store);
        let root = gid(1);
        let middle = gid(2);
        let leaf = gid(3);
        repo.nest(&root, &middle).unwrap();
        repo.nest(&middle, &leaf).unwrap();
        assert!(repo.is_descendant_of(&leaf, &root).unwrap());
        assert!(repo.is_descendant_of(&leaf, &middle).unwrap());
        assert!(!repo.is_descendant_of(&root, &leaf).unwrap());
        assert!(!repo.is_descendant_of(&root, &root).unwrap());
    }

    #[test]
    fn nest_rejects_self_loop() {
        let store = test_store();
        let repo = NamespaceRepository::new(&store);
        let g = gid(1);
        assert!(repo.nest(&g, &g).is_err());
    }

    #[test]
    fn identity_returns_none_when_unset() {
        let store = test_store();
        let repo = NamespaceRepository::new(&store);
        assert!(repo.identity(&gid(1)).unwrap().is_none());
    }

    #[test]
    fn store_then_identity_round_trip() {
        let store = test_store();
        let repo = NamespaceRepository::new(&store);
        let ns_id = gid(1);
        let pk = PublicKey::from([0x42; 32]);
        let sk = [0xAB; 32];
        let sender = [0xCD; 32];

        repo.store_identity(&ns_id, &pk, &sk, &sender).unwrap();
        let (loaded_pk, loaded_sk, loaded_sender) = repo
            .identity(&ns_id)
            .unwrap()
            .expect("identity must round-trip");
        assert_eq!(loaded_pk, pk);
        assert_eq!(loaded_sk, sk);
        assert_eq!(loaded_sender, sender);
    }
}
