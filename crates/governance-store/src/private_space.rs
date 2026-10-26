//! Structure rules of private spaces.
//!
//! A private space is a `Restricted` group and everything below it; the namespace
//! root and its open folders are the open space. Spaces do not nest and a folder
//! never leaves one. Each rule reads only the parent tree and visibility, so every
//! replica applying the op reaches the same verdict.

use calimero_context_config::types::ContextGroupId;
use calimero_context_config::VisibilityMode;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use crate::{
    CapabilitiesRepository, NamespaceError, NamespaceRepository, PrivateSpaceError,
    MAX_NAMESPACE_DEPTH,
};

fn is_private(store: &Store, group: &ContextGroupId) -> EyreResult<bool> {
    Ok(
        CapabilitiesRepository::new(store).subgroup_visibility(group)?
            == VisibilityMode::Restricted,
    )
}

/// The nearest private group on the chain from `start` up to, but excluding, the namespace root.
fn private_on_chain(store: &Store, start: &ContextGroupId) -> EyreResult<Option<ContextGroupId>> {
    let namespaces = NamespaceRepository::new(store);
    let mut current = *start;
    for _ in 0..=MAX_NAMESPACE_DEPTH {
        let Some(parent) = namespaces.parent(&current)? else {
            return Ok(None);
        };
        if is_private(store, &current)? {
            return Ok(Some(current));
        }
        current = parent;
    }
    bail!(NamespaceError::DepthExceeded)
}

fn private_below(store: &Store, group: &ContextGroupId) -> EyreResult<Option<ContextGroupId>> {
    for descendant in NamespaceRepository::new(store).collect_descendants(group)? {
        if is_private(store, &descendant)? {
            return Ok(Some(descendant));
        }
    }
    Ok(None)
}

fn holds_a_space(store: &Store, group: &ContextGroupId) -> EyreResult<bool> {
    Ok(is_private(store, group)? || private_below(store, group)?.is_some())
}

/// A new group under `parent` may be born private only outside every space.
pub fn require_may_create(
    store: &Store,
    parent: &ContextGroupId,
    restricted: bool,
) -> EyreResult<()> {
    if restricted && private_on_chain(store, parent)?.is_some() {
        bail!(PrivateSpaceError::Nested(parent.to_string()));
    }
    Ok(())
}

/// `group` may become private only outside every space and with no space below it.
/// The namespace root has no visibility to make private, so it is not judged.
pub fn require_may_make_private(store: &Store, group: &ContextGroupId) -> EyreResult<()> {
    let namespaces = NamespaceRepository::new(store);
    let Some(parent) = namespaces.parent(group)? else {
        return Ok(());
    };
    if private_on_chain(store, &parent)?.is_some() || private_below(store, group)?.is_some() {
        bail!(PrivateSpaceError::Nested(group.to_string()));
    }
    Ok(())
}

/// A move may not nest spaces and may not take a folder out of its space.
pub fn require_may_reparent(
    store: &Store,
    child: &ContextGroupId,
    new_parent: &ContextGroupId,
) -> EyreResult<()> {
    let Some(old_parent) = NamespaceRepository::new(store).parent(child)? else {
        return Ok(());
    };
    if old_parent == *new_parent {
        return Ok(());
    }
    let source = private_on_chain(store, &old_parent)?;
    let destination = private_on_chain(store, new_parent)?;
    if source.is_some() && source != destination {
        bail!(PrivateSpaceError::LeavesSpace(child.to_string()));
    }
    if destination.is_some() && holds_a_space(store, child)? {
        bail!(PrivateSpaceError::Nested(child.to_string()));
    }
    Ok(())
}

/// A group is deleted after the spaces beneath it, never with them.
pub fn require_may_delete(store: &Store, group: &ContextGroupId) -> EyreResult<()> {
    if private_below(store, group)?.is_some() {
        bail!(PrivateSpaceError::HoldsSpace(group.to_string()));
    }
    Ok(())
}

/// An invitation admits to the workspace, so its target is the namespace root.
pub fn require_root_invitation_target(store: &Store, group: &ContextGroupId) -> EyreResult<()> {
    if NamespaceRepository::new(store).parent(group)?.is_some() {
        bail!(PrivateSpaceError::InvitationTargetNotRoot(
            group.to_string()
        ));
    }
    Ok(())
}

/// Members are rows of the namespace root or of a space root, nowhere else.
pub fn require_member_row_target(store: &Store, group: &ContextGroupId) -> EyreResult<()> {
    if NamespaceRepository::new(store).parent(group)?.is_some() && !is_private(store, group)? {
        bail!(PrivateSpaceError::MemberRowOutsideSpaceRoot(
            group.to_string()
        ));
    }
    Ok(())
}
