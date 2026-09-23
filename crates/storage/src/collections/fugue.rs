//! Tree-Fugue: a non-interleaving sequence CRDT.
//!
//! Algorithm 1 of Weidner, Gentle and Kleppmann, *The Art of the Fugue* (arXiv 2305.00583).
//! Pure: no storage, no `env`, no HLC; identifiers are minted by the caller.
//! A node is `(id, value, parent, side)`; document order is the depth-first in-order
//! traversal, left children by ascending id, then the node's own value, then right children.
//! Plain Fugue, not FugueMax: right siblings order by id, so the paper's Figure 7 case is
//! residual, pinned by `figure_7__right_siblings_order_by_id_not_by_right_origin`.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

/// Replica identifier.
pub type ReplicaId = u64;

/// Per-replica monotonically increasing counter.
pub type SeqNo = u32;

/// A non-root node identifier, `(replicaID, counter)`.
pub type RawId = (ReplicaId, SeqNo);

/// A node identifier; `None` is the root.
pub type NodeId = Option<RawId>;

/// Which side of its parent a node hangs off; `L` sorts before `R`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    L,
    R,
}

/// A single Fugue tree node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FugueNode {
    pub id: RawId,
    /// The character; `None` is a tombstone.
    pub value: Option<char>,
    pub parent: NodeId,
    pub side: Side,
}

/// Errors raised by the local (non-effector) operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FugueError {
    IndexOutOfBounds { index: usize, len: usize },
    DuplicateId(RawId),
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

/// The Fugue tree; the root is implicit ([`NodeId`] `None`) and never stored.
#[derive(Clone, Debug, Default)]
pub struct FugueTree {
    nodes: BTreeMap<RawId, FugueNode>,
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

    /// The node with `id`, if it is attached.
    #[must_use]
    pub fn node(&self, id: RawId) -> Option<&FugueNode> {
        self.nodes.get(&id)
    }

    /// Every attached node, in id order.
    pub fn nodes(&self) -> impl Iterator<Item = &FugueNode> {
        self.nodes.values()
    }

    /// Whether the node is attached to the tree, live or tombstoned.
    #[must_use]
    pub fn contains(&self, id: RawId) -> bool {
        self.nodes.contains_key(&id)
    }

    fn children_of(&self, parent: NodeId, side: Side) -> impl Iterator<Item = RawId> + '_ {
        self.children
            .get(&(parent, side))
            .into_iter()
            .flat_map(|s| s.iter().copied())
    }

    /// Iterative, not recursive: a sequential append builds a right spine `n` deep.
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

    /// Every attached node's id in document order, tombstones included.
    #[cfg(test)]
    pub(super) fn ordered_ids(&self) -> Vec<RawId> {
        self.traverse_all().into_iter().flatten().collect()
    }

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

    /// The document text, tombstones skipped.
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

    /// Insert `value` at `index` under `id`, returning the node to broadcast.
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
            // `left_origin` has a right child, so its successor in the traversal exists.
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

    /// Tombstone the character at `index`, returning the id to broadcast.
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

    /// The insert effector: idempotent, order-independent, and delete-wins.
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

/// The paper's Figure 3: the list `abcdef`, with `a` and `b` both left children of `c`.
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

#[cfg(test)]
mod tests {
    use rand::rngs::StdRng;
    use rand::seq::SliceRandom;
    use rand::SeedableRng;

    use super::*;

    fn node_set(tree: &FugueTree) -> Vec<FugueNode> {
        tree.nodes().copied().collect()
    }

    #[test]
    fn values__figure_3_tree_reads_abcdef() {
        let tree = figure_3_tree();
        assert_eq!(tree.values(), "abcdef");
        assert_eq!(tree.len(), 6);
    }

    #[test]
    fn values__figure_3_is_order_independent() {
        let reference = figure_3_tree();
        let mut reversed = FugueTree::new();
        for node in node_set(&reference).into_iter().rev() {
            reversed.integrate(node);
        }
        assert_eq!(reversed.values(), "abcdef");
    }

    #[test]
    fn insert__sequential_typing_appends() {
        let mut tree = FugueTree::new();
        let _ = tree.insert(0, 'a', (1, 0)).unwrap();
        let _ = tree.insert(1, 'b', (1, 1)).unwrap();
        let _ = tree.insert(2, 'c', (1, 2)).unwrap();
        assert_eq!(tree.values(), "abc");
        assert_eq!(tree.node((1, 1)).unwrap().parent, Some((1, 0)));
        assert_eq!(tree.node((1, 1)).unwrap().side, Side::R);
        assert_eq!(tree.node((1, 2)).unwrap().parent, Some((1, 1)));
    }

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

        assert!(tree.contains((1, 1)));
        assert_eq!(tree.node((1, 1)).unwrap().value, None);

        let node = tree.insert(1, 'x', (1, 3)).unwrap();
        assert_eq!(node.parent, Some((1, 1)));
        assert_eq!(node.side, Side::L);
        assert_eq!(tree.values(), "axc");
        assert_eq!(tree.node((1, 1)).unwrap().value, None);
    }

    /// Seeded with the shared `S`, appends `passage`, then inserts `heading` before it.
    fn backward_insertion_replica(replica: ReplicaId, heading: char, passage: &str) -> FugueTree {
        let mut tree = FugueTree::new();
        tree.integrate(FugueNode {
            id: (0, 0),
            value: Some('S'),
            parent: None,
            side: Side::R,
        });

        let mut seq: SeqNo = 0;
        for ch in passage.chars() {
            let at = tree.len();
            let _ = tree.insert(at, ch, (replica, seq)).unwrap();
            seq += 1;
        }
        let _ = tree.insert(1, heading, (replica, seq)).unwrap();
        tree
    }

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

        assert!(
            merged.contains("Aaaa"),
            "alice's block interleaved: {merged}"
        );
        assert!(merged.contains("Bbbb"), "bob's block interleaved: {merged}");
        assert_eq!(merged.len(), "SAaaaBbbb".len());
        assert_eq!(merged, "SAaaaBbbb");
    }

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
        let mut rng = StdRng::seed_from_u64(0x5eed_1234);
        for round in 0..64 {
            let mut order = all.clone();
            match round {
                0 => {}
                1 => order.reverse(),
                _ => order.shuffle(&mut rng),
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
