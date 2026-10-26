//! Structure rules of private spaces.
//!
//! A private space is a `Restricted` group and everything below it; the namespace
//! root and its open folders are the open space. Spaces do not nest and a folder
//! never leaves one. The rules read a [`Tree`]: at apply time the tree at the op's
//! causal cut, so every replica reaches the same verdict; in a handler the live tree.

use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::VisibilityMode;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use crate::authorizer::{ensure_cut_resolvable, AtCutAuthorizer, SubgroupEdges, TreeReads};
use crate::{
    ApplyError, CapabilitiesRepository, NamespaceError, NamespaceRepository, PrivateSpaceError,
    MAX_NAMESPACE_DEPTH,
};

/// A namespace's folders: `child -> (parent, restricted)`; the root has no entry.
pub struct Tree(SubgroupEdges);

impl Tree {
    /// The tree at the cut of an op of `group`, judged on the groups in `reads`. A cut the authorizer cannot answer is undecidable
    /// unless there is no cut to resolve (empty, or no projection), which reads live.
    pub fn at_cut(
        store: &Store,
        authorizer: &dyn AtCutAuthorizer,
        group: &ContextGroupId,
        parents: &[[u8; 32]],
        reads: TreeReads<'_>,
    ) -> EyreResult<Self> {
        let namespace = NamespaceRepository::new(store).resolve(group)?;
        if let Some(edges) = authorizer.subgroup_edges_at_cut(&namespace, parents, reads) {
            return Ok(Self(edges));
        }
        ensure_cut_resolvable(authorizer, group, parents, "private space structure")?;
        if !parents.is_empty() && authorizer.reads_tree_at_cut() {
            bail!(ApplyError::AuthorityUndecidable {
                group_id: group.to_string(),
                signer: "private space structure".to_owned(),
            });
        }
        Self::live(store, group)
    }

    /// The tree as this node holds it now, for refusing early in a handler.
    pub fn live(store: &Store, group: &ContextGroupId) -> EyreResult<Self> {
        let namespaces = NamespaceRepository::new(store);
        let root = namespaces.resolve(group)?;
        let capabilities = CapabilitiesRepository::new(store);
        let mut edges = SubgroupEdges::new();
        for child in namespaces.collect_descendants(&root)? {
            if let Some(parent) = namespaces.parent(&child)? {
                let restricted =
                    capabilities.subgroup_visibility(&child)? == VisibilityMode::Restricted;
                let _ = edges.insert(child, (parent, restricted));
            }
        }
        Ok(Self(edges))
    }

    pub fn contains(&self, group: &ContextGroupId) -> bool {
        self.0.contains_key(group)
    }

    /// Whether `group` is a private space root or a folder inside one.
    pub fn in_space(&self, group: &ContextGroupId) -> EyreResult<bool> {
        Ok(self.space_root(group)?.is_some())
    }

    /// The root of the private space `group` is in, or is the root of.
    pub fn space_root(&self, group: &ContextGroupId) -> EyreResult<Option<ContextGroupId>> {
        self.private_on_chain(group)
    }

    fn is_private(&self, group: &ContextGroupId) -> bool {
        self.0.get(group).is_some_and(|(_, restricted)| *restricted)
    }

    /// The nearest private group from `start` up to, but excluding, the namespace root.
    fn private_on_chain(&self, start: &ContextGroupId) -> EyreResult<Option<ContextGroupId>> {
        let mut current = *start;
        for _ in 0..=MAX_NAMESPACE_DEPTH {
            let Some((parent, restricted)) = self.0.get(&current) else {
                return Ok(None);
            };
            if *restricted {
                return Ok(Some(current));
            }
            current = *parent;
        }
        bail!(NamespaceError::DepthExceeded)
    }

    fn is_below(&self, group: &ContextGroupId, ancestor: &ContextGroupId) -> EyreResult<bool> {
        let mut current = *group;
        for _ in 0..=MAX_NAMESPACE_DEPTH {
            let Some((parent, _)) = self.0.get(&current) else {
                return Ok(false);
            };
            if parent == ancestor {
                return Ok(true);
            }
            current = *parent;
        }
        bail!(NamespaceError::DepthExceeded)
    }

    fn private_below(&self, group: &ContextGroupId) -> EyreResult<bool> {
        for (child, (_, restricted)) in &self.0 {
            if *restricted && self.is_below(child, group)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A new group under `parent` may be born private only outside every space.
    pub fn require_may_create(&self, parent: &ContextGroupId, restricted: bool) -> EyreResult<()> {
        if restricted && self.private_on_chain(parent)?.is_some() {
            bail!(PrivateSpaceError::Nested(parent.to_string()));
        }
        Ok(())
    }

    /// `group` may become private only outside every space and with no space below it.
    /// The namespace root has no visibility to make private, so it is not judged.
    pub fn require_may_make_private(&self, group: &ContextGroupId) -> EyreResult<()> {
        let Some((parent, _)) = self.0.get(group) else {
            return Ok(());
        };
        if self.private_on_chain(parent)?.is_some() || self.private_below(group)? {
            bail!(PrivateSpaceError::Nested(group.to_string()));
        }
        Ok(())
    }

    /// A move may not nest spaces and may not take a folder out of its space: the rule the
    /// fold applies too.
    pub fn require_may_reparent(
        &self,
        child: &ContextGroupId,
        new_parent: &ContextGroupId,
    ) -> EyreResult<()> {
        match calimero_authz::reparent_refusal(&self.0, child, new_parent) {
            Ok(()) => Ok(()),
            Err(calimero_authz::ReparentRefusal::LeavesSpace) => {
                bail!(PrivateSpaceError::LeavesSpace(child.to_string()))
            }
            Err(calimero_authz::ReparentRefusal::Nested) => {
                bail!(PrivateSpaceError::Nested(child.to_string()))
            }
            Err(calimero_authz::ReparentRefusal::DepthExceeded) => {
                bail!(NamespaceError::DepthExceeded)
            }
        }
    }

    /// Whether `op` in `group` is sealed under the workspace key: the group is readable by every
    /// member. A tree op is judged by the parent chain, so a flip is seen by those who see the wall.
    pub fn sealed_for_workspace(&self, group: &ContextGroupId, op: &GroupOp) -> EyreResult<bool> {
        let Some((parent, _)) = self.0.get(group) else {
            return Ok(false);
        };
        let start = if is_tree_op(op) { parent } else { group };
        Ok(self.private_on_chain(start)?.is_none())
    }

    /// A folder is deleted after the spaces beneath it, never with them. The namespace root
    /// is the whole workspace and goes with everything in it.
    pub fn require_may_delete(&self, group: &ContextGroupId) -> EyreResult<()> {
        if self.0.contains_key(group) && self.private_below(group)? {
            bail!(PrivateSpaceError::HoldsSpace(group.to_string()));
        }
        Ok(())
    }

    /// Members are rows of the namespace root or of a space root, nowhere else.
    pub fn require_member_row_target(&self, group: &ContextGroupId) -> EyreResult<()> {
        if self.0.contains_key(group) && !self.is_private(group) {
            bail!(PrivateSpaceError::MemberRowOutsideSpaceRoot(
                group.to_string()
            ));
        }
        Ok(())
    }
}

/// Ops that change the tree every member reads: a visibility flip and a deletion.
pub fn is_tree_op(op: &GroupOp) -> bool {
    matches!(
        op.innermost(),
        GroupOp::SubgroupVisibilitySet { .. } | GroupOp::GroupDelete
    )
}

/// An invitation admits to the workspace, so its target is the namespace root.
pub fn require_root_invitation_target(
    namespace: &ContextGroupId,
    group: &ContextGroupId,
) -> EyreResult<()> {
    if group != namespace {
        bail!(PrivateSpaceError::InvitationTargetNotRoot(
            group.to_string()
        ));
    }
    Ok(())
}
