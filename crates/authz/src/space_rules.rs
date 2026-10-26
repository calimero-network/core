//! The structure rule a move of a folder obeys, shared by the live apply and the fold.

use std::collections::BTreeMap;

use calimero_context_config::types::ContextGroupId;

const MAX_NAMESPACE_DEPTH: usize = calimero_context_config::MAX_NAMESPACE_DEPTH;

/// A namespace's folders: `child -> (parent, restricted)`; the root has no entry.
pub type SpaceEdges = BTreeMap<ContextGroupId, (ContextGroupId, bool)>;

/// Why a move is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReparentRefusal {
    /// The folder would leave its private space, or enter another.
    LeavesSpace,
    /// A space would end up inside a space.
    Nested,
    /// The tree is deeper than the namespace allows.
    DepthExceeded,
}

/// The nearest private group from `start` up to, but excluding, the namespace root.
pub fn private_on_chain(
    edges: &SpaceEdges,
    start: &ContextGroupId,
) -> Result<Option<ContextGroupId>, ReparentRefusal> {
    let mut current = *start;
    for _ in 0..=MAX_NAMESPACE_DEPTH {
        let Some((parent, restricted)) = edges.get(&current) else {
            return Ok(None);
        };
        if *restricted {
            return Ok(Some(current));
        }
        current = *parent;
    }
    Err(ReparentRefusal::DepthExceeded)
}

fn is_below(
    edges: &SpaceEdges,
    group: &ContextGroupId,
    ancestor: &ContextGroupId,
) -> Result<bool, ReparentRefusal> {
    let mut current = *group;
    for _ in 0..=MAX_NAMESPACE_DEPTH {
        let Some((parent, _)) = edges.get(&current) else {
            return Ok(false);
        };
        if parent == ancestor {
            return Ok(true);
        }
        current = *parent;
    }
    Err(ReparentRefusal::DepthExceeded)
}

/// Whether `group` is a space root or holds one below it.
pub fn holds_a_space(edges: &SpaceEdges, group: &ContextGroupId) -> Result<bool, ReparentRefusal> {
    if edges.get(group).is_some_and(|(_, restricted)| *restricted) {
        return Ok(true);
    }
    for (child, (_, restricted)) in edges {
        if *restricted && is_below(edges, child, group)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A move may not nest spaces and may not take a folder out of its space.
pub fn reparent_refusal(
    edges: &SpaceEdges,
    child: &ContextGroupId,
    new_parent: &ContextGroupId,
) -> Result<(), ReparentRefusal> {
    let Some((old_parent, _)) = edges.get(child) else {
        return Ok(());
    };
    if old_parent == new_parent {
        return Ok(());
    }
    let source = private_on_chain(edges, old_parent)?;
    let destination = private_on_chain(edges, new_parent)?;
    if source.is_some() && source != destination {
        return Err(ReparentRefusal::LeavesSpace);
    }
    if destination.is_some() && holds_a_space(edges, child)? {
        return Err(ReparentRefusal::Nested);
    }
    Ok(())
}
