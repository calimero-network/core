use std::collections::HashSet;

use crate::{MembershipRepository, MetaRepository, NamespaceError};
use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    GroupChildIndex, GroupParentRef, NamespaceParticipation, NamespaceParticipationValue,
    NodeIdentity, NodeIdentityValue, GROUP_CHILD_INDEX_PREFIX, NAMESPACE_PARTICIPATION_PREFIX,
};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use rand::rngs::OsRng;
use rand::Rng;
use sha2::Digest;

use super::super::{
    cascade_remove_member_from_group_tree, collect_keys_with_prefix, get_group_for_context,
};

/// Re-exported from `calimero-context-config` — the single source of truth for
/// the namespace parent-chain walk bound (see `context_config::MAX_NAMESPACE_DEPTH`).
pub const MAX_NAMESPACE_DEPTH: usize = calimero_context_config::MAX_NAMESPACE_DEPTH;

#[derive(Debug, Clone, Copy)]
pub struct NamespaceIdentityRecord {
    pub public_key: PublicKey,
    pub private_key: [u8; 32],
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
    /// `identity` is a signing key, because every caller is holding one off a
    /// delta it just authenticated. The role it carries belongs to the ACCOUNT
    /// that key speaks for, so the key is resolved here rather than at each of
    /// the seven call sites.
    ///
    /// A key that resolves to nothing is reported as not read-only. That is not
    /// a fail-open: this is a negative gate consulted alongside a positive
    /// membership check, and an unresolvable author fails that one, so the
    /// combined verdict is still a refusal.
    pub fn is_read_only_for_context(
        &self,
        context_id: &ContextId,
        identity: &PublicKey,
    ) -> EyreResult<bool> {
        let Some(group_id) = get_group_for_context(self.store, context_id)? else {
            return Ok(false);
        };
        let Some(identity) = crate::member_account_in_namespace(self.store, &group_id, identity)?
        else {
            return Ok(false);
        };
        match MembershipRepository::new(self.store).role_of(&group_id, &identity)? {
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

        // Authority belongs to the account, so the executor's key is resolved
        // before any of the three lookups below. A key bound to no account here
        // is refused outright — it holds no grant under any principal this
        // group knows, and inventing a key-derived one would only manufacture a
        // principal that matches nothing.
        let Some(executor) = crate::member_account_in_namespace(self.store, &group_id, executor)?
        else {
            return Ok(false);
        };

        if MembershipRepository::new(self.store).is_admin(&group_id, &executor)? {
            return Ok(true);
        }

        if let Some(role) = MembershipRepository::new(self.store).role_of(&group_id, &executor)? {
            return Ok(matches!(
                role,
                calimero_primitives::context::GroupMemberRole::Admin
                    | calimero_primitives::context::GroupMemberRole::Member,
            ));
        }

        match MembershipRepository::new(self.store).check_path(&group_id, &executor)? {
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
            bail!(NamespaceError::SelfNesting);
        }

        if self.parent(child_group_id)?.is_some() {
            bail!(NamespaceError::AlreadyHasParent(format!(
                "{child_group_id:?}"
            )));
        }

        let mut current = *parent_group_id;
        let mut depth = 0usize;
        while let Some(ancestor) = self.parent(&current)? {
            if ancestor == *child_group_id {
                bail!(NamespaceError::NestingCycle);
            }
            depth += 1;
            if depth > MAX_NAMESPACE_DEPTH {
                bail!(NamespaceError::DepthExceeded);
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
        let mut visited = HashSet::new();
        let _ = visited.insert(*group_id);
        let mut stack = vec![*group_id];

        while let Some(current) = stack.pop() {
            let children = self.list_children(&current)?;
            for child in children {
                // Cycle/diamond guard: a malformed child index could point
                // back at an ancestor; skip already-seen groups so the walk
                // stays bounded (mirrors `cascade::walk_for_predicate`).
                if !visited.insert(child) {
                    continue;
                }
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
        viewer: &AccountId,
    ) -> EyreResult<Vec<ContextGroupId>> {
        let mut descendants = Vec::new();
        let mut visited = HashSet::new();
        let _ = visited.insert(*group_id);
        let mut stack = vec![*group_id];

        while let Some(current) = stack.pop() {
            for child in self.list_children(&current)? {
                // Cycle/diamond guard (see `collect_descendants`).
                if !visited.insert(child) {
                    continue;
                }
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

        // The invitation names the inviter twice: by key, because the joiner
        // verifies a signature, and by account, because the joiner seeds
        // governance rows from it before it can resolve anything itself.
        // Visibility is decided per account too.
        let inviter_account = crate::member_account_in_namespace(
            self.store,
            root_group_id,
            &inviter_sk.public_key(),
        )?
        .ok_or_else(|| {
            eyre::eyre!(
                "cannot issue invitations: the inviter's identity is bound to no account in \
                 namespace {root_group_id:?}"
            )
        })?;
        let mut groups = vec![*root_group_id];
        groups.extend(self.collect_visible_descendants(root_group_id, &inviter_account)?);

        let inviter_signer_id = SignerId::from(*inviter_sk.public_key());
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Saturate to avoid overflowing into a past/wrapped expiration when a
        // caller passes a very large `expiration_secs`.
        let expiration = now_secs.saturating_add(expiration_secs);

        let mut result = Vec::with_capacity(groups.len());
        for gid in groups {
            let invitation_nonce: [u8; 32] = OsRng.gen();

            let invitation = GroupInvitationFromAdmin {
                inviter_identity: inviter_signer_id,
                group_id: gid,
                expiration_timestamp: expiration,
                invitation_nonce,
                invited_role,
            };

            let inv_bytes = borsh::to_vec(&invitation).map_err(|e| eyre::eyre!("borsh: {e}"))?;
            let hash = sha2::Sha256::digest(&inv_bytes);
            let sig = inviter_sk
                .sign(&hash)
                .map_err(|e| eyre::eyre!("signing: {e}"))?;

            let (application_id, bytecode_id) = match MetaRepository::new(self.store).load(&gid)? {
                Some(meta) => (
                    Some(*meta.target_application_id.as_ref()),
                    Some(meta.bytecode_id),
                ),
                None => {
                    tracing::warn!(
                        group_id = %hex::encode(gid.to_bytes()),
                        "create_recursive_invitations: missing GroupMeta for descendant; \
                         issuing invitation with application_id + bytecode_id = None \
                         (joiner will fall back to zero)"
                    );
                    (None, None)
                }
            };

            let signed = SignedGroupOpenInvitation {
                invitation,
                inviter_signature: hex::encode(sig.to_bytes()),
                inviter_account: Some(inviter_account),
                application_id,
                bytecode_id,
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
        member: &AccountId,
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
        // Inclusive bound (mirrors `check_path`): reaching the root at depth D
        // requires D+1 iterations to observe the root's `None` parent, so the
        // deepest legal subgroup (depth MAX) needs MAX+1 walk steps.
        for _ in 0..=MAX_NAMESPACE_DEPTH {
            match self.parent(&current)? {
                Some(parent) => current = parent,
                None => return Ok(current),
            }
        }
        eyre::bail!(NamespaceError::DepthExceeded)
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
        let mut visited = std::collections::HashSet::new();
        visited.insert(*root);
        let mut stack = vec![*root];
        while let Some(g) = stack.pop() {
            for child in self.list_children(&g)? {
                if !visited.insert(child) {
                    continue;
                }
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
        let old_parent = self
            .parent(child)?
            .ok_or_else(|| NamespaceError::RootHasNoParent(format!("{child:?}")))?;

        if old_parent == *new_parent {
            return Ok(ReparentOutcome::Unchanged);
        }

        if MetaRepository::new(self.store).load(new_parent)?.is_none() {
            eyre::bail!(NamespaceError::ReparentTargetMissing(format!(
                "{new_parent:?}"
            )));
        }

        // Both must live in the same namespace. Meta rows are keyed by group id
        // alone, so the target-exists check above does not prove same-namespace;
        // without this an admin of namespace A could reparent an A-group under a
        // B-group, grafting A's crypto/access boundary into B.
        if self.resolve(child)? != self.resolve(new_parent)? {
            eyre::bail!(NamespaceError::ReparentCrossNamespace {
                child: format!("{child:?}"),
                new_parent: format!("{new_parent:?}"),
            });
        }

        if self.is_descendant_of(new_parent, child)? {
            eyre::bail!(NamespaceError::ReparentCycle {
                new_parent: format!("{new_parent:?}"),
                child: format!("{child:?}"),
            });
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
        eyre::bail!(NamespaceError::DepthExceeded)
    }

    /// The key this node signs with **in `namespace_id`**.
    ///
    /// `None` when the node does not take part there.
    pub fn identity(
        &self,
        namespace_id: &ContextGroupId,
    ) -> EyreResult<Option<crate::ResolvedIdentity>> {
        Ok(self
            .identity_record(namespace_id)?
            .map(|record| (record.public_key, record.private_key)))
    }

    /// As [`Self::identity`], as a record.
    ///
    /// **The namespace gate is the point.** The keypair itself is node-level — one
    /// node signs with one key — so reading it alone would answer "yes, here it is"
    /// for every namespace on earth. Callers throughout the tree read this `None`
    /// as "not my namespace": it decides whether to emit a readiness beacon, whether
    /// to self-report a migration, whether a self-purge already ran. Answering from
    /// the keypair would have every one of them fire for namespaces this node has
    /// never joined, so participation is checked first and the key second.
    pub fn identity_record(
        &self,
        namespace_id: &ContextGroupId,
    ) -> EyreResult<Option<NamespaceIdentityRecord>> {
        if !self.participates_in(namespace_id)? {
            return Ok(None);
        }
        let handle = self.store.handle();
        match handle.get(&NodeIdentity::new())? {
            Some(val) => Ok(Some(NamespaceIdentityRecord {
                public_key: PublicKey::from(val.public_key),
                private_key: val.private_key,
            })),
            None => Ok(None),
        }
    }

    /// Persist the node's signing identity, and note that it takes part in
    /// `namespace_id`.
    ///
    /// Two writes because they answer two questions. The keypair is node-level
    /// and idempotent after the first namespace. The marker is per namespace and
    /// is what `participating_namespaces` walks — a node has to know which namespaces to
    /// sync, and nothing else records that.
    pub fn store_identity(
        &self,
        namespace_id: &ContextGroupId,
        public_key: &PublicKey,
        private_key: &[u8; 32],
    ) -> EyreResult<()> {
        // Refuse a second, different key rather than overwriting. There is one
        // row now, so a caller storing a namespace-specific key — which the old
        // per-namespace model allowed — would silently replace the key every
        // OTHER namespace signs with, and the damage would surface later as
        // signatures nobody can attribute. A node has one signing key; storing a
        // different one is a bug in the caller, not a state to reconcile.
        if let Some(existing) = self.store.handle().get(&NodeIdentity::new())? {
            if existing.public_key != **public_key {
                eyre::bail!(
                    "refusing to replace this node's signing key: it already holds \
                     {} and was asked to store {} for {namespace_id:?}. One node signs \
                     with one key",
                    PublicKey::from(existing.public_key),
                    public_key,
                );
            }
            return self.note_participation(namespace_id);
        }

        let mut handle = self.store.handle();
        handle.put(
            &NodeIdentity::new(),
            &NodeIdentityValue {
                public_key: **public_key,
                private_key: *private_key,
            },
        )?;
        self.note_participation(namespace_id)
    }

    /// Repoint this node at a different signing key, discarding the one it holds.
    ///
    /// Deliberate re-provisioning, which [`Self::store_identity`] refuses on
    /// purpose: with one key per node, an overwrite reached by accident silently
    /// changes what every namespace signs with. Everything authored under the old
    /// key stays attributed to it — this does not re-sign history — so the node
    /// is a different member afterwards wherever the old key was the member.
    pub fn replace_identity(
        &self,
        namespace_id: &ContextGroupId,
        public_key: &PublicKey,
        private_key: &[u8; 32],
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.put(
            &NodeIdentity::new(),
            &NodeIdentityValue {
                public_key: **public_key,
                private_key: *private_key,
            },
        )?;
        self.note_participation(namespace_id)
    }

    /// The key this node signs with, regardless of scope.
    ///
    /// [`Self::identity`] gates the same key on participation, because its
    /// callers are asking "who am I *here*" and `None` has to mean "not my
    /// namespace". This one answers the node-level question directly, for a
    /// caller that has no namespace in hand — reporting what the node is, rather
    /// than what it is within something.
    pub fn node_identity(&self) -> EyreResult<Option<NamespaceIdentityRecord>> {
        let handle = self.store.handle();
        match handle.get(&NodeIdentity::new())? {
            Some(val) => Ok(Some(NamespaceIdentityRecord {
                public_key: PublicKey::from(val.public_key),
                private_key: val.private_key,
            })),
            None => Ok(None),
        }
    }

    /// Whether this node takes part in `namespace_id`.
    ///
    /// Distinct from holding a signing key: the key is node-level and outlives
    /// eviction from any one namespace, so "do I have a key" stopped being the
    /// same question as "am I in this namespace".
    pub fn participates_in(&self, namespace_id: &ContextGroupId) -> EyreResult<bool> {
        Ok(self
            .store
            .handle()
            .get(&NamespaceParticipation::new(namespace_id.to_bytes()))?
            .is_some())
    }

    /// Record that this node takes part in `namespace_id`.
    ///
    /// Idempotent, and carries no key material — the row's presence is its whole
    /// meaning.
    pub fn note_participation(&self, namespace_id: &ContextGroupId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.put(
            &NamespaceParticipation::new(namespace_id.to_bytes()),
            &NamespaceParticipationValue { reserved: 0 },
        )?;
        Ok(())
    }

    /// Enumerate every namespace this node holds an identity for.
    ///
    /// A `NamespaceParticipation` row is written exactly once per namespace the
    /// node has joined (`store_identity`), so this is the node's full set of
    /// known namespaces. Range-scans the shared `Group` column by the
    /// `NAMESPACE_PARTICIPATION_PREFIX` byte — the same seek-and-walk convention
    /// `collect_keys_with_prefix` uses everywhere else in this crate, which
    /// terminates at the first key whose leading byte differs (the next key
    /// type in the shared column), not on corruption.
    ///
    /// Used by the #2848 Part C curative startup sweep to drive a buffered-op
    /// re-drive across every namespace the node already participates in.
    /// Every namespace this node takes part in.
    ///
    /// It was `iter_identities`, which described the old model — one identity
    /// per namespace. There is one identity now, so what this enumerates is
    /// namespaces, and the name says so.
    pub fn participating_namespaces(&self) -> EyreResult<Vec<ContextGroupId>> {
        let keys = collect_keys_with_prefix(
            self.store,
            NamespaceParticipation::new([0u8; 32]),
            NAMESPACE_PARTICIPATION_PREFIX,
            |_k| true,
        )?;
        Ok(keys
            .into_iter()
            .map(|k| ContextGroupId::from(k.namespace_id()))
            .collect())
    }

    /// Resolve the namespace for a group and return this node's identity.
    pub fn resolve_identity(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<Option<crate::ResolvedIdentity>> {
        Ok(self
            .resolve_identity_record(group_id)?
            .map(|record| (record.public_key, record.private_key)))
    }

    pub fn resolve_identity_record(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<Option<NamespaceIdentityRecord>> {
        let ns_id = self.resolve(group_id)?;
        self.identity_record(&ns_id)
    }

    /// Record that this node takes part in the namespace containing `group_id`,
    /// and return the one key it signs with — minting it on first use.
    ///
    /// It was `get_or_create_identity`, which read as "an identity per group".
    /// There is one identity: the create half mints the NODE's keypair, once
    /// ever, and the per-group half is only a participation marker. Naming the
    /// marker is the point, because this WRITES — it is not a way to ask what
    /// this node signs with. For that see [`Self::resolve_identity`], which
    /// reads and answers `None` rather than enlisting the node.
    pub fn participate_in(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<(ContextGroupId, PublicKey, [u8; 32])> {
        let bundle = self.participate_in_bundle(group_id)?;
        Ok((
            bundle.namespace_id,
            bundle.identity.public_key,
            bundle.identity.private_key,
        ))
    }

    pub fn participate_in_bundle(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<ResolvedNamespaceIdentity> {
        let ns_id = self.resolve(group_id)?;

        // Unconditionally, and BEFORE the early return. The signing key is
        // node-level, so from the second namespace onward the branch below exits
        // with one already in hand — and skipping this would leave every namespace
        // after the first unmarked, so `participating_namespaces` would report only the one
        // the node happened to join first and it would sync nothing else.
        self.note_participation(&ns_id)?;

        if let Some(identity) = self.identity_record(&ns_id)? {
            return Ok(ResolvedNamespaceIdentity {
                namespace_id: ns_id,
                identity,
            });
        }

        let private_key = PrivateKey::random(&mut OsRng);
        let public_key = private_key.public_key();

        self.store_identity(&ns_id, &public_key, private_key.as_bytes())?;

        Ok(ResolvedNamespaceIdentity {
            namespace_id: ns_id,
            identity: NamespaceIdentityRecord {
                public_key,
                private_key: *private_key.as_bytes(),
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
    use crate::test_fixtures::test_store;

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

        repo.store_identity(&ns_id, &pk, &sk).unwrap();
        let (loaded_pk, loaded_sk) = repo
            .identity(&ns_id)
            .unwrap()
            .expect("identity must round-trip");
        assert_eq!(loaded_pk, pk);
        assert_eq!(loaded_sk, sk);
    }

    /// One node signs with one key, in every namespace it takes part in.
    #[test]
    fn the_signing_identity_is_shared_across_namespaces() {
        let store = test_store();
        let repo = NamespaceRepository::new(&store);
        let (ns_a, ns_b) = (
            ContextGroupId::from([0xAAu8; 32]),
            ContextGroupId::from([0xBBu8; 32]),
        );

        let a = repo.participate_in_bundle(&ns_a).expect("first");
        let b = repo.participate_in_bundle(&ns_b).expect("second");

        assert_eq!(
            a.identity.public_key, b.identity.public_key,
            "a second namespace must not mint a second signing key — the key is \
             recorded as the device's sign_pk, and a device has one"
        );
        assert_eq!(a.identity.private_key, b.identity.private_key);
    }

    /// Every namespace is enumerable, not just the first.
    ///
    /// The signing key is node-level, so from the second namespace onward
    /// `participate_in_bundle` finds one already stored and returns early.
    /// If the participation marker were written on the create path only, that early
    /// return would leave every later namespace unrecorded — and `participating_namespaces`
    /// drives which namespaces `join_context` syncs and which ones the startup
    /// buffered-op sweep re-drives, so the node would silently stop servicing all
    /// but the first namespace it ever joined.
    #[test]
    fn joining_a_second_namespace_is_still_recorded() {
        let store = test_store();
        let repo = NamespaceRepository::new(&store);
        let (ns_a, ns_b) = (
            ContextGroupId::from([0xAAu8; 32]),
            ContextGroupId::from([0xBBu8; 32]),
        );

        let _ = repo.participate_in_bundle(&ns_a).expect("first");
        let _ = repo.participate_in_bundle(&ns_b).expect("second");

        let mut seen = repo.participating_namespaces().expect("enumerate");
        seen.sort();
        let mut want = vec![ns_a, ns_b];
        want.sort();
        assert_eq!(
            seen, want,
            "both namespaces must be enumerable, not only the one that minted the key"
        );
    }
}
