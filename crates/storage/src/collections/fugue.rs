//! Tree-Fugue: a non-interleaving sequence CRDT.
//!
//! Transcription of Algorithm 1 (Tree-Fugue) from Weidner, Gentle and
//! Kleppmann, *The Art of the Fugue* (arXiv 2305.00583; IEEE TPDS 2025).
//!
//! This module is **pure**: no storage, no `env`, no HLC. Identifiers are
//! minted by the caller and passed in. It is a data structure, not a service.
//!
//! ## The algorithm
//!
//! A node is `(id, value, parent, side)` where `side ∈ {L, R}` and `value` is
//! either a character or the tombstone. The tree always contains a root node
//! `(null, ⊥, null, null)`. Document order is the depth-first **in-order**
//! traversal: left children (ordered by id ascending), then the node's own
//! value, then right children (ordered by id ascending). Tombstoned nodes are
//! skipped when reading values but are still traversed and can still be
//! parents.
//!
//! `insert(i, x)` takes `leftOrigin` = the node holding the `(i-1)`-th value
//! (the root when `i == 0`). If `leftOrigin` has no right child, the new node
//! is its right child. Otherwise `rightOrigin` is the node immediately after
//! `leftOrigin` in the traversal **including tombstones**, and the new node is
//! `rightOrigin`'s left child.
//!
//! `delete(i)` sets the node's value to the tombstone. The node itself stays,
//! because it may be an ancestor of live nodes.
//!
//! ## Example
//!
//! ```ignore
//! use calimero_storage::collections::fugue::FugueTree;
//!
//! let mut tree = FugueTree::new();
//! tree.insert(0, 'a', (1, 0)).unwrap();
//! tree.insert(1, 'b', (1, 1)).unwrap();
//! tree.insert(1, 'c', (1, 2)).unwrap();
//! assert_eq!(tree.values(), "acb");
//! ```

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

pub mod key;

/// Replica identifier.
pub type ReplicaId = u64;

/// Per-replica monotonically increasing counter.
pub type SeqNo = u32;

/// A non-root node identifier, `(replicaID, counter)`.
///
/// Ordering is lexicographic on the tuple, which is exactly the ordering of
/// the big-endian byte concatenation used by [`key::path_key`].
pub type RawId = (ReplicaId, SeqNo);

/// A node identifier. `None` is the root — the paper's `null`.
pub type NodeId = Option<RawId>;

/// Which side of its parent a node hangs off.
///
/// `L` sorts before `R` so that `(parent, side)` keys group left children
/// before right children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    /// Left child: ordered *before* the parent's own value.
    L,
    /// Right child: ordered *after* the parent's own value.
    R,
}

/// A single Fugue tree node: the paper's `(id, value, parent, side)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FugueNode {
    /// This node's identifier.
    pub id: RawId,
    /// The character, or `None` for the tombstone (`⊥`).
    pub value: Option<char>,
    /// The parent node, `None` meaning the root.
    pub parent: NodeId,
    /// Which side of `parent` this node hangs off.
    pub side: Side,
}

/// Errors raised by the local (non-effector) operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FugueError {
    /// The requested index is past the end of the document.
    IndexOutOfBounds {
        /// The index that was asked for.
        index: usize,
        /// The number of live characters at the time of the call.
        len: usize,
    },
    /// The caller minted an id that already exists in the tree.
    DuplicateId(RawId),
    /// The referenced node is not in the tree.
    UnknownNode(RawId),
}

impl fmt::Display for FugueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::IndexOutOfBounds { index, len } => {
                write!(f, "index {index} out of bounds for length {len}")
            }
            Self::DuplicateId(id) => write!(f, "duplicate node id {id:?}"),
            Self::UnknownNode(id) => write!(f, "unknown node id {id:?}"),
        }
    }
}

impl core::error::Error for FugueError {}

/// The Fugue tree.
///
/// The root is implicit: it is [`NodeId`] `None` and is never stored in
/// `nodes`. Nodes whose parent has not been delivered yet are buffered and
/// integrated automatically once the parent arrives, which makes
/// [`FugueTree::integrate`] order-independent.
#[derive(Clone, Debug, Default)]
pub struct FugueTree {
    /// All delivered, attached nodes, keyed by id.
    nodes: BTreeMap<RawId, FugueNode>,
    /// `(parent, side) -> children`, each set ordered by id ascending.
    children: BTreeMap<(NodeId, Side), BTreeSet<RawId>>,
    /// Nodes waiting on a not-yet-delivered parent, keyed by that parent.
    pending: BTreeMap<RawId, Vec<FugueNode>>,
}

impl FugueTree {
    /// A tree containing only the root.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The node with the given id, if it is attached to the tree.
    #[must_use]
    pub fn node(&self, id: RawId) -> Option<&FugueNode> {
        self.nodes.get(&id)
    }

    /// Every attached node, in id order.
    pub fn nodes(&self) -> impl Iterator<Item = &FugueNode> {
        self.nodes.values()
    }

    /// Whether the node is attached to the tree (live *or* tombstoned).
    #[must_use]
    pub fn contains(&self, id: RawId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// The children of `parent` on `side`, in ascending id order.
    fn children_of(&self, parent: NodeId, side: Side) -> impl Iterator<Item = RawId> + '_ {
        self.children
            .get(&(parent, side))
            .into_iter()
            .flat_map(|s| s.iter().copied())
    }

    /// The full depth-first in-order traversal, **including** the root and
    /// tombstoned nodes.
    ///
    /// Iterative rather than recursive: a sequential append builds a right
    /// spine of depth `n`, so recursion would blow the stack on real
    /// documents.
    fn traverse_all(&self) -> Vec<NodeId> {
        enum Frame {
            Visit(NodeId),
            Emit(NodeId),
        }

        let mut out = Vec::with_capacity(self.nodes.len() + 1);
        let mut stack = vec![Frame::Visit(None)];
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Emit(id) => out.push(id),
                Frame::Visit(id) => {
                    let right: Vec<RawId> = self.children_of(id, Side::R).collect();
                    for child in right.into_iter().rev() {
                        stack.push(Frame::Visit(Some(child)));
                    }
                    stack.push(Frame::Emit(id));
                    let left: Vec<RawId> = self.children_of(id, Side::L).collect();
                    for child in left.into_iter().rev() {
                        stack.push(Frame::Visit(Some(child)));
                    }
                }
            }
        }
        out
    }

    /// The id of the `n`-th live (non-tombstoned) node in document order.
    fn nth_live(&self, order: &[NodeId], n: usize) -> Option<RawId> {
        let mut seen = 0;
        for id in order {
            let Some(raw) = *id else { continue };
            if self
                .nodes
                .get(&raw)
                .is_some_and(|node| node.value.is_some())
            {
                if seen == n {
                    return Some(raw);
                }
                seen += 1;
            }
        }
        None
    }

    /// The document text: the in-order traversal, tombstones skipped.
    #[must_use]
    pub fn values(&self) -> String {
        self.traverse_all()
            .into_iter()
            .filter_map(|id| self.nodes.get(&id?))
            .filter_map(|node| node.value)
            .collect()
    }

    /// The number of live characters.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.values().filter(|n| n.value.is_some()).count()
    }

    /// Whether the document is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Insert `value` at `index`, minting the node with the caller-supplied
    /// `id`.
    ///
    /// Returns the node that must be broadcast, so the caller can ship the
    /// effector without re-deriving it.
    pub fn insert(
        &mut self,
        index: usize,
        value: char,
        id: RawId,
    ) -> Result<FugueNode, FugueError> {
        if self.nodes.contains_key(&id) {
            return Err(FugueError::DuplicateId(id));
        }

        let order = self.traverse_all();
        let left_origin: NodeId = if index == 0 {
            None
        } else {
            Some(
                self.nth_live(&order, index - 1)
                    .ok_or(FugueError::IndexOutOfBounds {
                        index,
                        len: self.len(),
                    })?,
            )
        };

        let node = if self.children_of(left_origin, Side::R).next().is_none() {
            FugueNode {
                id,
                value: Some(value),
                parent: left_origin,
                side: Side::R,
            }
        } else {
            // `leftOrigin` has a right child, so its successor in the
            // tombstone-inclusive traversal is the leftmost node of that right
            // subtree and therefore always exists.
            let pos = order
                .iter()
                .position(|n| *n == left_origin)
                .expect("left origin is in the traversal");
            let right_origin = order[pos + 1];
            FugueNode {
                id,
                value: Some(value),
                parent: right_origin,
                side: Side::L,
            }
        };

        self.integrate(node);
        Ok(node)
    }

    /// Tombstone the character at `index`.
    ///
    /// Returns the id that must be broadcast. The node stays in the tree.
    pub fn delete(&mut self, index: usize) -> Result<RawId, FugueError> {
        let order = self.traverse_all();
        let id = self
            .nth_live(&order, index)
            .ok_or(FugueError::IndexOutOfBounds {
                index,
                len: self.len(),
            })?;
        self.tombstone(id)?;
        Ok(id)
    }

    /// The delete effector: tombstone a node by id.
    pub fn tombstone(&mut self, id: RawId) -> Result<(), FugueError> {
        let node = self.nodes.get_mut(&id).ok_or(FugueError::UnknownNode(id))?;
        node.value = None;
        Ok(())
    }

    /// The insert effector: apply a node from a remote replica.
    ///
    /// Idempotent and order-independent. A node whose parent has not arrived
    /// yet is buffered until it does. Re-delivering a node that is already
    /// present is a no-op, except that a tombstoned copy wins over a live one
    /// (deletes are delete-wins).
    pub fn integrate(&mut self, node: FugueNode) {
        let mut queue = vec![node];
        while let Some(node) = queue.pop() {
            if let Some(parent) = node.parent {
                if !self.nodes.contains_key(&parent) {
                    self.pending.entry(parent).or_default().push(node);
                    continue;
                }
            }

            let id = node.id;
            if let Some(existing) = self.nodes.get_mut(&id) {
                if node.value.is_none() {
                    existing.value = None;
                }
            } else {
                let _ignored = self
                    .children
                    .entry((node.parent, node.side))
                    .or_default()
                    .insert(id);
                let _ignored = self.nodes.insert(id, node);
            }

            if let Some(waiting) = self.pending.remove(&id) {
                queue.extend(waiting);
            }
        }
    }
}

/// The tree of Figure 3 of the paper: the list `abcdef` in which `a` and `b`
/// are both **left** children of `c`, ordered between themselves by id.
///
/// ```text
///          root
///            \ R
///             c (1,2)
///        L  /  \  R
///   (1,0) a     d (1,3)
///   (1,1) b        \ R
///                   e (1,4)
///                      \ R
///                       f (1,5)
/// ```
#[cfg(test)]
pub(super) fn figure_3_tree() -> FugueTree {
    let mut tree = FugueTree::new();
    for node in [
        FugueNode {
            id: (1, 2),
            value: Some('c'),
            parent: None,
            side: Side::R,
        },
        FugueNode {
            id: (1, 0),
            value: Some('a'),
            parent: Some((1, 2)),
            side: Side::L,
        },
        FugueNode {
            id: (1, 1),
            value: Some('b'),
            parent: Some((1, 2)),
            side: Side::L,
        },
        FugueNode {
            id: (1, 3),
            value: Some('d'),
            parent: Some((1, 2)),
            side: Side::R,
        },
        FugueNode {
            id: (1, 4),
            value: Some('e'),
            parent: Some((1, 3)),
            side: Side::R,
        },
        FugueNode {
            id: (1, 5),
            value: Some('f'),
            parent: Some((1, 4)),
            side: Side::R,
        },
    ] {
        tree.integrate(node);
    }
    tree
}

/// A deterministic xorshift64* PRNG, so the randomised tests are reproducible
/// without pulling a dependency into the storage crate.
#[cfg(test)]
pub(super) struct Rng(u64);

#[cfg(test)]
impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub(super) fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub(super) fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            usize::try_from(self.next_u64() % bound as u64).unwrap_or(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every node delivered to `tree`, as a flat list, for replaying into
    /// another replica.
    fn node_set(tree: &FugueTree) -> Vec<FugueNode> {
        tree.nodes().copied().collect()
    }

    /// (a) Figure 3 of the paper reads back as `abcdef`.
    #[test]
    fn values__figure_3_tree_reads_abcdef() {
        let tree = figure_3_tree();
        assert_eq!(tree.values(), "abcdef");
        assert_eq!(tree.len(), 6);
    }

    /// (a') The same tree integrated in a different order is the same tree.
    #[test]
    fn values__figure_3_is_order_independent() {
        let reference = figure_3_tree();
        let mut reversed = FugueTree::new();
        for node in node_set(&reference).into_iter().rev() {
            reversed.integrate(node);
        }
        assert_eq!(reversed.values(), "abcdef");
    }

    /// (b) Sequential typing.
    #[test]
    fn insert__sequential_typing_appends() {
        let mut tree = FugueTree::new();
        let _ = tree.insert(0, 'a', (1, 0)).unwrap();
        let _ = tree.insert(1, 'b', (1, 1)).unwrap();
        let _ = tree.insert(2, 'c', (1, 2)).unwrap();
        assert_eq!(tree.values(), "abc");

        // The paper's insert rule makes each append the right child of its
        // predecessor: a right spine.
        assert_eq!(tree.node((1, 1)).unwrap().parent, Some((1, 0)));
        assert_eq!(tree.node((1, 1)).unwrap().side, Side::R);
        assert_eq!(tree.node((1, 2)).unwrap().parent, Some((1, 1)));
    }

    /// (c) Inserting in the middle becomes a left child of the right origin.
    #[test]
    fn insert__mid_document_yields_acb() {
        let mut tree = FugueTree::new();
        let _ = tree.insert(0, 'a', (1, 0)).unwrap();
        let _ = tree.insert(1, 'b', (1, 1)).unwrap();
        let node = tree.insert(1, 'c', (1, 2)).unwrap();
        assert_eq!(tree.values(), "acb");
        assert_eq!(node.parent, Some((1, 1)), "c hangs off the right origin b");
        assert_eq!(node.side, Side::L);
    }

    /// (d) Delete tombstones in place; the node remains and can still parent.
    #[test]
    fn delete__tombstones_but_keeps_node_as_parent() {
        let mut tree = FugueTree::new();
        let _ = tree.insert(0, 'a', (1, 0)).unwrap();
        let _ = tree.insert(1, 'b', (1, 1)).unwrap();
        let _ = tree.insert(2, 'c', (1, 2)).unwrap();

        let deleted = tree.delete(1).unwrap();
        assert_eq!(deleted, (1, 1));
        assert_eq!(tree.values(), "ac");
        assert_eq!(tree.len(), 2);

        // The tombstoned node is still in the tree.
        assert!(tree.contains((1, 1)));
        assert_eq!(tree.node((1, 1)).unwrap().value, None);

        // ... and is still reachable as a parent: `a` has a right child, so
        // the right origin is the tombstone itself.
        let node = tree.insert(1, 'x', (1, 3)).unwrap();
        assert_eq!(node.parent, Some((1, 1)));
        assert_eq!(node.side, Side::L);
        assert_eq!(tree.values(), "axc");
        assert_eq!(tree.node((1, 1)).unwrap().value, None);
    }

    /// Build a replica seeded with `S`, then append `passage` and insert
    /// `heading` immediately before it. Returns the replica.
    fn backward_insertion_replica(replica: ReplicaId, heading: char, passage: &str) -> FugueTree {
        let mut tree = FugueTree::new();
        // The shared, already-synchronised seed document.
        tree.integrate(FugueNode {
            id: (0, 0),
            value: Some('S'),
            parent: None,
            side: Side::R,
        });

        let mut seq: SeqNo = 0;
        // Append the passage at the end of the document.
        for ch in passage.chars() {
            let at = tree.len();
            let _ = tree.insert(at, ch, (replica, seq)).unwrap();
            seq += 1;
        }
        // Go back and insert the heading immediately before our own passage.
        let _ = tree.insert(1, heading, (replica, seq)).unwrap();
        tree
    }

    /// (e) THE HEADLINE TEST: backward insertion does not interleave.
    ///
    /// Paper Figure 2 / Table 1: RGA is proven to interleave here; Fugue is
    /// proven not to.
    #[test]
    fn merge__backward_insertion_does_not_interleave() {
        let alice = backward_insertion_replica(1, 'A', "aaa");
        let bob = backward_insertion_replica(2, 'B', "bbb");
        assert_eq!(alice.values(), "SAaaa");
        assert_eq!(bob.values(), "SBbbb");

        let mut alice_then_bob = alice.clone();
        for node in node_set(&bob) {
            alice_then_bob.integrate(node);
        }
        let mut bob_then_alice = bob.clone();
        for node in node_set(&alice) {
            bob_then_alice.integrate(node);
        }

        let merged = alice_then_bob.values();
        assert_eq!(merged, bob_then_alice.values(), "merge must be commutative");

        // Each heading is immediately followed by its own passage, and each
        // passage is contiguous. That is the non-interleaving property.
        assert!(
            merged.contains("Aaaa"),
            "alice's block interleaved: {merged}"
        );
        assert!(merged.contains("Bbbb"), "bob's block interleaved: {merged}");
        assert_eq!(merged.len(), "SAaaaBbbb".len());
        assert_eq!(merged, "SAaaaBbbb");
    }

    /// (f) Convergence: any delivery order of the same node set converges.
    #[test]
    fn integrate__converges_under_every_delivery_order() {
        let alice = backward_insertion_replica(1, 'A', "aaa");
        let bob = backward_insertion_replica(2, 'B', "bbb");
        let carol = backward_insertion_replica(3, 'C', "cc");

        let mut all = node_set(&alice);
        all.extend(node_set(&bob));
        all.extend(node_set(&carol));
        all.sort_by_key(|n| n.id);
        all.dedup_by_key(|n| n.id);

        let mut reference: Option<String> = None;
        let mut rng = Rng::new(0x5eed_1234);
        for round in 0..64 {
            let mut order = all.clone();
            // Round 0 is ascending, round 1 descending, the rest shuffled.
            match round {
                0 => {}
                1 => order.reverse(),
                _ => {
                    for i in (1..order.len()).rev() {
                        order.swap(i, rng.below(i + 1));
                    }
                }
            }

            let mut tree = FugueTree::new();
            for node in order {
                tree.integrate(node);
            }
            let text = tree.values();
            match &reference {
                None => reference = Some(text),
                Some(expected) => assert_eq!(&text, expected, "divergence at round {round}"),
            }
        }
        let converged = reference.unwrap();
        assert!(converged.contains("Aaaa"));
        assert!(converged.contains("Bbbb"));
        assert!(converged.contains("Ccc"));
    }

    /// Out-of-range indices are refused rather than silently clamped.
    #[test]
    fn insert__rejects_out_of_range_index() {
        let mut tree = FugueTree::new();
        let _ = tree.insert(0, 'a', (1, 0)).unwrap();
        assert_eq!(
            tree.insert(5, 'b', (1, 1)),
            Err(FugueError::IndexOutOfBounds { index: 5, len: 1 })
        );
        assert_eq!(
            tree.insert(0, 'b', (1, 0)),
            Err(FugueError::DuplicateId((1, 0)))
        );
        assert_eq!(
            tree.delete(3),
            Err(FugueError::IndexOutOfBounds { index: 3, len: 1 })
        );
    }
}
