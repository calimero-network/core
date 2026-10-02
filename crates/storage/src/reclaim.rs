//! What tombstone GC may reclaim from the raw rows of a context's state.
//!
//! A delete leaves two records behind: the deleted entity's tombstone row
//! (its index, with `deleted_at` set and no data) and the entity's id in its
//! parent's `deleted_children`. The tombstone may be collected once every
//! member of the context has applied the delete, which the node decides
//! (`calimero-node`'s `tombstone_stability`); this module says only which rows
//! are tombstones it may collect at all ([`tombstone_deleted_at`]). The id in
//! the parent exists only to point at that tombstone: the sync wire resolves
//! each id to the child's tombstone and skips an id whose tombstone is gone.
//! Once the tombstone has been collected, the id is inert, and
//! [`prune_deleted_children`] drops it. Both records then go in the same
//! sweep.
//!
//! These functions read and produce whole physical entity rows
//! ([`crate::row`]), so a node can call them on its store without running the
//! storage layer.

use borsh::to_vec;

use crate::address::Id;
use crate::row::{decode, encode, Row};

/// The `deleted_at` of the row of entity `id`, if it is a tombstone GC may
/// collect once every member has applied the delete.
///
/// A value qualifies only if it decodes as an entity row whose index carries a
/// `deleted_at` AND re-encodes to the exact same bytes. The row codec refuses
/// every form it never produces, so GC never deletes a row that merely happens
/// to decode. A terminal tombstone (a written-once entry's) never qualifies:
/// it is what keeps its owner's key deleted, and is kept for good.
///
/// Deleting the row deletes the whole entity row. A tombstone's row holds only
/// its index record, since the delete removed the data, so nothing live goes
/// with it.
#[must_use]
pub fn tombstone_deleted_at(id: Id, bytes: &[u8]) -> Option<u64> {
    let row = decode(id, bytes)?;
    let index = row.entity_index()?;
    let deleted_at = index.deleted_at?;
    if index.is_terminal_tombstone() || encode(id, &row) != bytes {
        return None;
    }
    Some(deleted_at)
}

/// The row of entity `id` with every `deleted_children` entry whose child row
/// is gone dropped, or `None` if no entry is dropped (or the row is not an
/// entity row this layer wrote).
///
/// `has_row` says whether a child still has a row. A child with any row stays
/// listed: its tombstone has not been collected yet, or it is a terminal one,
/// or it has been re-added (which drops it from the list anyway).
#[must_use]
pub fn prune_deleted_children(
    id: Id,
    bytes: &[u8],
    mut has_row: impl FnMut(Id) -> bool,
) -> Option<Vec<u8>> {
    let row = decode(id, bytes)?;
    let mut index = row.entity_index()?;
    if index.deleted_children.is_empty() || encode(id, &row) != bytes {
        return None;
    }
    let listed = index.deleted_children.len();
    index.deleted_children.retain(|child| has_row(*child));
    if index.deleted_children.len() == listed {
        return None;
    }
    let pruned = Row {
        index: Some(to_vec(&index).ok()?),
        data: row.data,
    };
    Some(encode(id, &pruned))
}

/// Whether the row of entity `id` lists any deleted children, so a GC pass
/// should revisit it with [`prune_deleted_children`] once tombstones are gone.
#[must_use]
pub fn lists_deleted_children(id: Id, bytes: &[u8]) -> bool {
    decode(id, bytes)
        .and_then(|row| row.entity_index())
        .is_some_and(|index| !index.deleted_children.is_empty())
}

#[cfg(test)]
#[path = "tests/reclaim.rs"]
mod tests;
