//! `FugueTextSimple` - Tree-Fugue with one storage entity per node.
//!
//! A MEASUREMENT CONTROL, not a product collection, behind the off-by-default
//! `fugue-simple` feature (and `cfg(test)` for this crate's own tests). It
//! splits the win measured for [`FugueText`](super::FugueText) over
//! [`ReplicatedGrowableArray`](super::ReplicatedGrowableArray) into its two
//! causes: RGA to `FugueTextSimple` isolates Fugue's ordering rule,
//! `FugueTextSimple` to `FugueText` isolates run-length blocks. Everything but
//! the entity layout is `FugueText`'s unchanged, ordering included (both
//! delegate to the pure [`fugue`](super::fugue) module), so a cost difference
//! between the two can only be attributed to blocks.
//!
//! Entries deliberately carry no leaf `CrdtType`: tagging would mean a new
//! variant, a new dispatch arm and a new published enum value, which is the
//! permanent maintenance surface this type must not acquire. A node entity is
//! mutated once in its life, by the delete that tombstones it, so the untagged
//! last-writer-wins fallback can drop a tombstone; the cost workloads never
//! delete, and [`merge_nodes_from`](FugueTextSimple::merge_nodes_from)
//! implements the delete-wins join for the in-crate convergence tests.

use borsh::{BorshDeserialize, BorshSerialize};

use super::fugue::{FugueNode, FugueTree, Side};
use super::{CrdtType, UnorderedMap};
use crate::collections::error::StoreError;
use crate::env;
use crate::store::{MainStorage, StorageAdaptor};

/// Identifier of a single Fugue node: `(replica, counter)`.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
pub(crate) struct NodeId {
    /// The replica that minted this node.
    replica: u64,
    /// The replica-local counter value.
    counter: u32,
}

impl NodeId {
    const fn raw(self) -> (u64, u32) {
        (self.replica, self.counter)
    }

    const fn from_raw((replica, counter): (u64, u32)) -> Self {
        Self { replica, counter }
    }
}

/// Storage key for a node (owns serialized bytes for `AsRef<[u8]>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeKey {
    id: NodeId,
    bytes: Vec<u8>,
}

impl BorshSerialize for NodeKey {
    fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
        self.id.serialize(writer)
    }
}

impl BorshDeserialize for NodeKey {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        let id = NodeId::deserialize_reader(reader)?;
        let bytes = borsh::to_vec(&id).map_err(borsh::io::Error::other)?;
        Ok(Self { id, bytes })
    }
}

impl NodeKey {
    fn new(id: NodeId) -> Self {
        // `NodeId` is a fixed-size POD struct; serialization cannot fail in
        // practice. `unwrap_or_default` is a safety fallback only.
        let bytes = borsh::to_vec(&id).unwrap_or_default();
        Self { id, bytes }
    }

    const fn id(&self) -> NodeId {
        self.id
    }
}

impl AsRef<[u8]> for NodeKey {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// Borsh-serializable mirror of [`Side`], for the same reason
/// `fugue_text::BlockSide` exists: `fugue.rs` is deliberately storage-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub(crate) enum NodeSide {
    /// Left child, ordered before the parent's own value.
    L,
    /// Right child, ordered after the parent's own value.
    R,
}

impl From<Side> for NodeSide {
    fn from(side: Side) -> Self {
        match side {
            Side::L => Self::L,
            Side::R => Self::R,
        }
    }
}

impl From<NodeSide> for Side {
    fn from(side: NodeSide) -> Self {
        match side {
            NodeSide::L => Self::L,
            NodeSide::R => Self::R,
        }
    }
}

/// One Fugue node, stored as one entity. The whole point of this type.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub(crate) struct TextNode {
    /// The character, as `u32` for a fixed borsh layout.
    content: u32,
    /// The parent node; `None` is the Fugue root.
    parent: Option<NodeId>,
    /// Which side of `parent` this node hangs off.
    side: NodeSide,
    /// Whether this node has been deleted. A Fugue node must survive deletion,
    /// since a tombstone can still parent live nodes, so the entity is never
    /// removed. One entity covers one node here, so a bool suffices where
    /// `FugueText` needs a per-node bitmap.
    deleted: bool,
}

impl TextNode {
    fn as_char(&self) -> char {
        char::from_u32(self.content).unwrap_or('\u{fffd}')
    }
}

/// Tree-Fugue text with one storage entity per node: the paper's Simple shape.
///
/// See the module documentation: this is a cost control for
/// [`FugueText`](super::FugueText), not a collection to build on.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct FugueTextSimple<S: StorageAdaptor = MainStorage> {
    /// Nodes, keyed by their own id.
    #[borsh(bound(serialize = "", deserialize = ""))]
    pub(crate) nodes: UnorderedMap<NodeKey, TextNode, S>,
}

impl FugueTextSimple<MainStorage> {
    /// Create a new empty document with a random ID.
    #[must_use]
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Create a new document with a deterministic ID derived from `field_name`.
    #[must_use]
    pub fn new_with_field_name(field_name: &str) -> Self {
        Self::new_with_field_name_internal(None, field_name)
    }
}

impl Default for FugueTextSimple<MainStorage> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: StorageAdaptor> FugueTextSimple<S> {
    fn new_internal() -> Self {
        Self {
            nodes: UnorderedMap::new_internal(),
        }
    }

    /// Create a new document with a deterministic ID (internal).
    pub(super) fn new_with_field_name_internal(
        parent_id: Option<crate::address::Id>,
        field_name: &str,
    ) -> Self {
        Self {
            nodes: UnorderedMap::new_with_field_name_and_crdt_type(
                parent_id,
                field_name,
                node_map_crdt_type(),
            ),
        }
    }

    /// Insert a character at the given visible position.
    ///
    /// # Panics
    /// Panics if called inside a state migration, for the reason
    /// [`FugueText::insert`](super::FugueText::insert) does: node ids are minted
    /// from the node-local device id.
    ///
    /// # Errors
    /// Returns an error if `pos` is out of bounds or storage fails.
    pub fn insert(&mut self, pos: usize, content: char) -> Result<(), StoreError> {
        self.insert_str(pos, content.encode_utf8(&mut [0_u8; 4]))
    }

    /// Insert a string at the given visible position.
    ///
    /// # Panics
    /// Panics inside a state migration; see [`insert`](Self::insert).
    ///
    /// # Errors
    /// Returns an error if `pos` is out of bounds or storage fails.
    #[expect(
        clippy::panic,
        reason = "non-deterministic during migrate (node-local device id); a loud panic is \
                  the intended, unmissable guard against a silent network divergence"
    )]
    pub fn insert_str(&mut self, pos: usize, s: &str) -> Result<(), StoreError> {
        if env::in_merge_mode() {
            panic!(
                "FugueTextSimple::insert_str() is non-deterministic during a state \
                 migration: it mints node ids from the node-local device id, diverging ids \
                 across nodes."
            );
        }
        let replica = local_replica();
        self.insert_str_with_replica(pos, replica, s)
    }

    /// Insert a string at `pos`, minting node ids under an explicit `replica`.
    ///
    /// The deterministic counterpart of [`insert_str`](Self::insert_str).
    ///
    /// Re-deriving the tree per character is deliberate: with one entity per
    /// node there is nothing to batch into, which is the cost this control
    /// exists to measure.
    ///
    /// # Errors
    /// Returns an error if `pos` is out of bounds or storage fails.
    pub fn insert_str_with_replica(
        &mut self,
        pos: usize,
        replica: u64,
        s: &str,
    ) -> Result<(), StoreError> {
        for (offset, content) in s.chars().enumerate() {
            self.insert_one(pos + offset, replica, content)?;
        }
        Ok(())
    }

    /// Delete the character at the given visible position.
    ///
    /// # Errors
    /// Returns an error if `pos` is out of bounds or storage fails.
    pub fn delete(&mut self, pos: usize) -> Result<(), StoreError> {
        self.delete_range(pos, pos.checked_add(1).ok_or_else(|| out_of_bounds(pos))?)
    }

    /// Delete the half-open range `start..end` of visible positions.
    ///
    /// Clamped in `end` exactly like `FugueText::delete_range`.
    ///
    /// # Errors
    /// Returns an error if `start > end` or storage fails.
    pub fn delete_range(&mut self, start: usize, end: usize) -> Result<(), StoreError> {
        if start > end {
            return Err(invalid("start must be <= end"));
        }

        let loaded = self.load()?;
        let mut tree = build_tree(&loaded);

        let count = end.min(tree.len()).saturating_sub(start);
        let mut targets: Vec<(u64, u32)> = Vec::with_capacity(count);
        for _ in 0..count {
            match tree.delete(start) {
                Ok(id) => targets.push(id),
                Err(_) => break,
            }
        }

        for raw in targets {
            let id = NodeId::from_raw(raw);
            let key = NodeKey::new(id);
            let mut node = self
                .nodes
                .get(&key)?
                .ok_or_else(|| invalid("deleted node has no entity"))?
                .into_inner();
            if !node.deleted {
                node.deleted = true;
                let _ignored = self.nodes.insert(key, node)?;
            }
        }

        Ok(())
    }

    /// The document text, tombstones excluded.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn get_text(&self) -> Result<String, StoreError> {
        let loaded = self.load()?;
        Ok(build_tree(&loaded).values())
    }

    /// The characters in the half-open range `start..end` (char indices).
    ///
    /// Clamped in `end`, like `FugueText::text_range`.
    ///
    /// # Errors
    /// Returns an error if `start > end` or storage fails.
    pub fn text_range(&self, start: usize, end: usize) -> Result<String, StoreError> {
        if start > end {
            return Err(invalid("start must be <= end"));
        }
        let loaded = self.load()?;
        let text = build_tree(&loaded).values();
        Ok(text.chars().skip(start).take(end - start).collect())
    }

    /// The character at `pos`, or `None` if `pos` is past the end.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn char_at(&self, pos: usize) -> Result<Option<char>, StoreError> {
        let loaded = self.load()?;
        Ok(build_tree(&loaded).values().chars().nth(pos))
    }

    /// The number of visible characters.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn len(&self) -> Result<usize, StoreError> {
        let loaded = self.load()?;
        Ok(build_tree(&loaded).len())
    }

    /// Whether the document has no visible characters.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        self.len().map(|len| len == 0)
    }

    /// Insert one character, minting `(replica, next_counter)` and writing it
    /// as its own entity. No coalescing: that is the whole difference.
    fn insert_one(&mut self, pos: usize, replica: u64, content: char) -> Result<(), StoreError> {
        let loaded = self.load()?;
        let mut tree = build_tree(&loaded);
        let counter = next_counter(replica, &loaded)?;

        let node = tree
            .insert(pos, content, (replica, counter))
            .map_err(|err| invalid(&err.to_string()))?;

        let id = NodeId::from_raw(node.id);
        let stored = TextNode {
            content: node.value.unwrap_or('\u{fffd}') as u32,
            parent: node.parent.map(NodeId::from_raw),
            side: node.side.into(),
            deleted: false,
        };
        let _ignored = self.nodes.insert(NodeKey::new(id), stored)?;
        Ok(())
    }

    /// Every stored node, ascending by id.
    fn load(&self) -> Result<Vec<(NodeId, TextNode)>, StoreError> {
        let mut loaded: Vec<(NodeId, TextNode)> = self
            .nodes
            .entries()?
            .map(|(key, node)| (key.id(), node))
            .collect();
        loaded.sort_by_key(|(id, _)| *id);
        Ok(loaded)
    }

    /// Copy every node from `other` that `self` does not already hold, and OR
    /// in incoming tombstones: the delete-wins join, and a lattice one. Generic
    /// over `S2` so a cross-store merge is testable.
    ///
    /// `cfg(test)`-only, because this control ships no leaf `CrdtType`, so only
    /// this crate's own convergence tests have any business calling it.
    #[cfg(test)]
    pub(crate) fn merge_nodes_from<S2: StorageAdaptor>(
        &mut self,
        other: &FugueTextSimple<S2>,
    ) -> Result<(), StoreError> {
        for (key, incoming) in other.nodes.entries()? {
            if let Some(mine) = self.nodes.get(&key)? {
                let mine = mine.into_inner();
                if incoming.deleted && !mine.deleted {
                    let _ignored = self.nodes.insert(key, incoming)?;
                }
                continue;
            }
            let _ignored = self.nodes.insert(key, incoming)?;
        }
        Ok(())
    }
}

/// The container's `CrdtType`.
///
/// A plain `UnorderedMap`, not a `FugueText`: the container is structured
/// storage, so the container arm's job is only to return the incoming bytes.
/// Claiming `CrdtType::FugueText` would route it to a dispatcher that
/// deserialises a `FugueText` from these bytes.
fn node_map_crdt_type() -> CrdtType {
    CrdtType::UnorderedMap
}

/// Expand the stored nodes into a Fugue tree: one entity, one node.
fn build_tree(loaded: &[(NodeId, TextNode)]) -> FugueTree {
    let mut tree = FugueTree::new();
    for (id, node) in loaded {
        tree.integrate(FugueNode {
            id: id.raw(),
            value: (!node.deleted).then(|| node.as_char()),
            parent: node.parent.map(NodeId::raw),
            side: node.side.into(),
        });
    }
    tree
}

/// The next unused counter for `replica`, one past every node of that replica
/// the stored state mentions, as a definition or as a `parent` edge.
///
/// `FugueText::next_counter`'s rule, for the same reason: a tombstoned node is
/// never removed, so the mark survives deletion and a counter is never reused.
fn next_counter(replica: u64, loaded: &[(NodeId, TextNode)]) -> Result<u32, StoreError> {
    let mut next: u64 = 0;
    for (id, node) in loaded {
        if id.replica == replica {
            next = next.max(u64::from(id.counter) + 1);
        }
        if let Some(parent) = node.parent {
            if parent.replica == replica {
                next = next.max(u64::from(parent.counter) + 1);
            }
        }
    }
    u32::try_from(next).map_err(|_| invalid("replica counter space exhausted"))
}

/// This node's replica id: the first 8 bytes of its device id, `FugueText`'s
/// rule unchanged.
fn local_replica() -> u64 {
    let device = env::device_id();
    let mut head = [0_u8; 8];
    head.copy_from_slice(&device[..8]);
    u64::from_be_bytes(head)
}

fn invalid(message: &str) -> StoreError {
    StoreError::StorageError(crate::interface::StorageError::InvalidData(message.into()))
}

fn out_of_bounds(pos: usize) -> StoreError {
    invalid(&format!("position {pos} out of bounds"))
}

#[cfg(test)]
mod tests {
    use super::FugueTextSimple;
    use crate::collections::{Root, UnorderedMap};
    use crate::env;
    use crate::store::StorageAdaptor;

    /// A document in an explicit storage scope with a deterministic collection
    /// id, so two scopes standing in for two replicas agree on entity ids.
    fn doc_in<S: StorageAdaptor>(field_name: &str) -> FugueTextSimple<S> {
        FugueTextSimple {
            nodes: UnorderedMap::new_with_field_name_and_crdt_type(
                None,
                field_name,
                super::node_map_crdt_type(),
            ),
        }
    }

    /// The defining property: `n` characters are `n` entities, where
    /// `FugueText`'s `insert_str__round_trips_as_a_single_block` asserts one.
    #[test]
    fn insert_str__stores_one_entity_per_character() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueTextSimple::new);
        doc.insert_str(0, "hello").unwrap();
        assert_eq!(doc.get_text().unwrap(), "hello");
        assert_eq!(doc.len().unwrap(), 5);
        assert_eq!(
            doc.nodes.len().unwrap(),
            5,
            "a five-character run must be five entities, not one"
        );
    }

    /// Sequential appends do NOT coalesce: the block optimisation is absent by
    /// construction.
    #[test]
    fn insert__sequential_appends_do_not_coalesce() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueTextSimple::new);
        doc.insert(0, 'a').unwrap();
        doc.insert(1, 'b').unwrap();
        doc.insert(2, 'c').unwrap();
        assert_eq!(doc.get_text().unwrap(), "abc");
        assert_eq!(doc.nodes.len().unwrap(), 3);
    }

    /// Mid-document insert and the positional reads.
    #[test]
    fn insert__mid_document_and_positional_reads() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueTextSimple::new);
        doc.insert_str(0, "hello").unwrap();
        doc.insert(2, 'X').unwrap();
        assert_eq!(doc.get_text().unwrap(), "heXllo");
        assert_eq!(doc.char_at(2).unwrap(), Some('X'));
        assert_eq!(doc.text_range(1, 4).unwrap(), "eXl");
        assert_eq!(doc.text_range(4, 99).unwrap(), "lo", "end clamps");
    }

    /// Deletion tombstones the entity in place; the node survives as a parent,
    /// so the entity count does not fall.
    #[test]
    fn delete__tombstones_the_entity_and_keeps_it() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueTextSimple::new);
        doc.insert_str(0, "hello").unwrap();
        doc.delete(2).unwrap();
        assert_eq!(doc.get_text().unwrap(), "helo");
        assert_eq!(doc.len().unwrap(), 4);
        assert_eq!(doc.nodes.len().unwrap(), 5, "the node survives as a parent");

        doc.delete_range(0, 99).unwrap();
        assert_eq!(doc.get_text().unwrap(), "");
        assert!(doc.is_empty().unwrap());
        assert_eq!(doc.nodes.len().unwrap(), 5);
    }

    /// A deterministic build is a pure function of its inputs: no clock, no
    /// node-local input, exactly as `FugueText`'s is.
    #[test]
    fn insert_str_with_replica__is_deterministic() {
        type A = crate::store::MockedStorage<881>;
        type B = crate::store::MockedStorage<882>;
        env::reset_for_testing();

        let mut a = doc_in::<A>("det-a");
        a.insert_str_with_replica(0, 7, "hello").unwrap();
        a.insert_str_with_replica(2, 7, "X").unwrap();
        let mut b = doc_in::<B>("det-b");
        b.insert_str_with_replica(0, 7, "hello").unwrap();
        b.insert_str_with_replica(2, 7, "X").unwrap();

        assert_eq!(a.get_text().unwrap(), b.get_text().unwrap());
        assert_eq!(a.get_text().unwrap(), "heXllo");
    }

    /// A replica seeded with the shared `S`, which appends `passage` and then
    /// goes back to insert `heading` immediately before its own passage.
    fn backward_insertion_doc<S: StorageAdaptor>(
        field_name: &str,
        replica: u64,
        heading: &str,
        passage: &str,
    ) -> FugueTextSimple<S> {
        let mut doc = doc_in::<S>(field_name);
        doc.insert_str_with_replica(0, 0, "S").unwrap();
        doc.insert_str_with_replica(1, replica, passage).unwrap();
        doc.insert_str_with_replica(1, replica, heading).unwrap();
        doc
    }

    /// The same headline property as `FugueText`'s, which is what makes the
    /// pair a controlled experiment: backward insertion does not interleave.
    #[test]
    fn merge__backward_insertion_does_not_interleave() {
        type A = crate::store::MockedStorage<883>;
        type B = crate::store::MockedStorage<884>;
        env::reset_for_testing();

        let alice = backward_insertion_doc::<A>("simple", 1, "A", "aaa");
        let bob = backward_insertion_doc::<B>("simple", 2, "B", "bbb");
        assert_eq!(alice.get_text().unwrap(), "SAaaa");
        assert_eq!(bob.get_text().unwrap(), "SBbbb");

        let mut alice_then_bob = doc_in::<A>("simple-ab");
        alice_then_bob.merge_nodes_from(&alice).unwrap();
        alice_then_bob.merge_nodes_from(&bob).unwrap();
        let mut bob_then_alice = doc_in::<B>("simple-ba");
        bob_then_alice.merge_nodes_from(&bob).unwrap();
        bob_then_alice.merge_nodes_from(&alice).unwrap();

        let merged = alice_then_bob.get_text().unwrap();
        assert_eq!(merged, bob_then_alice.get_text().unwrap());
        assert_eq!(merged, "SAaaaBbbb");
    }

    /// The merge join is delete-wins in both directions.
    #[test]
    fn merge__delete_wins_in_either_order() {
        type A = crate::store::MockedStorage<885>;
        type B = crate::store::MockedStorage<886>;
        env::reset_for_testing();

        let mut a = doc_in::<A>("del");
        a.insert_str_with_replica(0, 2, "abc").unwrap();
        let mut b = doc_in::<B>("del");
        b.insert_str_with_replica(0, 2, "abc").unwrap();
        a.delete(1).unwrap();

        let mut a_then_b = doc_in::<A>("del-ab");
        a_then_b.merge_nodes_from(&a).unwrap();
        a_then_b.merge_nodes_from(&b).unwrap();
        let mut b_then_a = doc_in::<B>("del-ba");
        b_then_a.merge_nodes_from(&b).unwrap();
        b_then_a.merge_nodes_from(&a).unwrap();

        assert_eq!(a_then_b.get_text().unwrap(), "ac");
        assert_eq!(b_then_a.get_text().unwrap(), "ac");
    }
}
