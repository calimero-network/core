//! `FugueText` — a storage-backed, run-length-blocked Tree-Fugue text CRDT.
//!
//! This is the storage collection that replaces
//! [`ReplicatedGrowableArray`](super::ReplicatedGrowableArray). It keeps RGA's
//! wiring — an [`UnorderedMap`] of immutable, id-keyed entities, so it inherits
//! the existing CRDT merge, tombstoning and re-key machinery — and swaps RGA's
//! ordering rule for Tree-Fugue's, which is proven non-interleaving on backward
//! insertion where RGA is proven to interleave.
//!
//! ## Representation
//!
//! The synced state is `(parent, side)` edges, exactly as the pure
//! [`fugue`](super::fugue) module models them — fixed-size and bounded, the same
//! shape RGA already ships in `left`.
//!
//! One entity is **one run**, not one character. A [`TextBlock`] holds
//! `text.chars().count()` consecutive Fugue nodes whose ids are
//! `(replica, counter), (replica, counter + 1), …`. The first node hangs off
//! `(parent, side)`; every later node `i` is the **right** child of node
//! `i - 1`. That is the single biggest lever on the document ceiling, and it is
//! the paper's own optimised Tree-Fugue (§4, "condenses sequentially-inserted
//! tree nodes into a single item object").
//!
//! Ordering is *computed*: the stored blocks are expanded into a
//! [`FugueTree`](super::fugue::FugueTree) and the tree is asked for the order.
//! Reads are therefore O(n); the ordered index is deliberately out of scope
//! here.
//!
//! ## Example
//!
//! ```ignore
//! use calimero_storage::collections::FugueText;
//!
//! let mut doc = FugueText::new();
//! doc.insert_str(0, "hello").unwrap();
//! doc.insert(2, 'X').unwrap();
//! assert_eq!(doc.get_text().unwrap(), "heXllo");
//! ```

use borsh::{BorshDeserialize, BorshSerialize};

use super::fugue::Side;
use super::{CrdtType, UnorderedMap};
use crate::collections::error::StoreError;
use crate::store::{MainStorage, StorageAdaptor};

/// Identifier of a single Fugue node: `(replica, counter)`.
///
/// Ordered lexicographically, matching [`fugue::RawId`](super::fugue::RawId).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
pub(crate) struct BlockId {
    /// The replica that minted this run.
    replica: u64,
    /// The first counter value of the run.
    counter: u32,
}

impl BlockId {
    const fn new(replica: u64, counter: u32) -> Self {
        Self { replica, counter }
    }

    const fn raw(self) -> (u64, u32) {
        (self.replica, self.counter)
    }

    const fn from_raw((replica, counter): (u64, u32)) -> Self {
        Self { replica, counter }
    }
}

/// Storage key for a block (owns serialized bytes for `AsRef<[u8]>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BlockKey {
    id: BlockId,
    bytes: Vec<u8>,
}

impl BorshSerialize for BlockKey {
    fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
        self.id.serialize(writer)
    }
}

impl BorshDeserialize for BlockKey {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        let id = BlockId::deserialize_reader(reader)?;
        let bytes = borsh::to_vec(&id).map_err(borsh::io::Error::other)?;
        Ok(Self { id, bytes })
    }
}

impl BlockKey {
    fn new(id: BlockId) -> Self {
        // `BlockId` is a fixed-size POD struct; serialization cannot fail in
        // practice. `unwrap_or_default` is a safety fallback only.
        let bytes = borsh::to_vec(&id).unwrap_or_default();
        Self { id, bytes }
    }

    const fn id(&self) -> BlockId {
        self.id
    }
}

impl AsRef<[u8]> for BlockKey {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// Borsh-serializable mirror of [`Side`].
///
/// `super::fugue::Side` is a pure-algorithm type with no serialization derives,
/// and `fugue.rs` is deliberately storage-free, so the wire form lives here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub(crate) enum BlockSide {
    /// Left child — ordered before the parent's own value.
    L,
    /// Right child — ordered after the parent's own value.
    R,
}

impl From<Side> for BlockSide {
    fn from(side: Side) -> Self {
        match side {
            Side::L => Self::L,
            Side::R => Self::R,
        }
    }
}

impl From<BlockSide> for Side {
    fn from(side: BlockSide) -> Self {
        match side {
            BlockSide::L => Self::L,
            BlockSide::R => Self::R,
        }
    }
}

/// One run of consecutive Fugue nodes stored as a single entity.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub(crate) struct TextBlock {
    /// The id of the run's first node. Duplicated from the map key so the value
    /// is self-describing to a host-side decoder.
    start_id: BlockId,
    /// The run's characters. Node `i` of the run holds the `i`-th character.
    text: String,
    /// The parent of the run's *first* node; `None` is the Fugue root.
    parent: Option<BlockId>,
    /// Which side of `parent` the run's first node hangs off.
    side: BlockSide,
    /// Whether every node in this run is tombstoned.
    ///
    /// Fugue nodes must survive deletion — a tombstone can still be the parent
    /// of live nodes — so a delete cannot drop the entity. It splits the run so
    /// the deleted region is exactly one block and flags that block.
    deleted: bool,
}

/// A storage-backed collaborative text collection with Tree-Fugue ordering.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct FugueText<S: StorageAdaptor = MainStorage> {
    /// Runs, keyed by the id of their first node.
    #[borsh(bound(serialize = "", deserialize = ""))]
    pub(crate) blocks: UnorderedMap<BlockKey, TextBlock, S>,
}

/// Re-key the block map relative to its storage parent so a `FugueText` stored
/// as a collection value converges across nodes. See [`super::rekey`].
impl<S: StorageAdaptor> super::rekey::RekeyTarget for FugueText<S> {
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        self.blocks.reassign_deterministic_id_under(
            parent_id,
            "__fugue_blocks",
            CrdtType::FugueText,
        );
        self.blocks.set_collection_crdt_type(CrdtType::FugueText);
    }
}

impl FugueText<MainStorage> {
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

impl Default for FugueText<MainStorage> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: StorageAdaptor> FugueText<S> {
    fn new_internal() -> Self {
        Self {
            blocks: UnorderedMap::new_internal(),
        }
    }

    /// Create a new document with a deterministic ID (internal).
    pub(super) fn new_with_field_name_internal(
        parent_id: Option<crate::address::Id>,
        field_name: &str,
    ) -> Self {
        Self {
            blocks: UnorderedMap::new_with_field_name_and_crdt_type(
                parent_id,
                field_name,
                CrdtType::FugueText,
            ),
        }
    }

    /// Reassign the collection's ID deterministically from the field name.
    ///
    /// Called by the `#[app::state]` macro after `init()`.
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        self.blocks.reassign_deterministic_id(field_name);
        self.blocks.set_collection_crdt_type(CrdtType::FugueText);
    }

    /// Insert a character at the given visible position.
    ///
    /// # Panics
    /// Panics inside a state migration (`#[app::migrate]`).
    ///
    /// # Errors
    /// Returns an error if `pos` is out of bounds or storage fails.
    pub fn insert(&mut self, _pos: usize, _content: char) -> Result<(), StoreError> {
        Err(unimplemented_err())
    }

    /// Insert a string at the given visible position.
    ///
    /// # Panics
    /// Panics inside a state migration (`#[app::migrate]`).
    ///
    /// # Errors
    /// Returns an error if `pos` is out of bounds or storage fails.
    pub fn insert_str(&mut self, _pos: usize, _s: &str) -> Result<(), StoreError> {
        Err(unimplemented_err())
    }

    /// Insert a string minting node ids under an explicit `replica`.
    ///
    /// # Errors
    /// Returns an error if `pos` is out of bounds or storage fails.
    pub fn insert_str_with_replica(
        &mut self,
        _pos: usize,
        _replica: u64,
        _s: &str,
    ) -> Result<(), StoreError> {
        Err(unimplemented_err())
    }

    /// Delete the character at the given visible position.
    ///
    /// # Errors
    /// Returns an error if `pos` is out of bounds or storage fails.
    pub fn delete(&mut self, _pos: usize) -> Result<(), StoreError> {
        Err(unimplemented_err())
    }

    /// Delete a half-open range of visible positions.
    ///
    /// # Errors
    /// Returns an error if `start > end` or storage fails.
    pub fn delete_range(&mut self, _start: usize, _end: usize) -> Result<(), StoreError> {
        Err(unimplemented_err())
    }

    /// The document text, tombstones excluded.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn get_text(&self) -> Result<String, StoreError> {
        Err(unimplemented_err())
    }

    /// The number of visible characters.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn len(&self) -> Result<usize, StoreError> {
        Err(unimplemented_err())
    }

    /// Whether the document has no visible characters.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        self.len().map(|len| len == 0)
    }

    /// Copy every block from `other` that `self` neither holds nor tombstoned.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub(crate) fn merge_blocks_from<S2: StorageAdaptor>(
        &mut self,
        _other: &FugueText<S2>,
    ) -> Result<(), StoreError> {
        Err(unimplemented_err())
    }
}

fn unimplemented_err() -> StoreError {
    StoreError::StorageError(crate::interface::StorageError::InvalidData(
        "FugueText is not implemented yet".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{BlockId, BlockKey, BlockSide, FugueText, TextBlock};
    use crate::collections::{Root, UnorderedMap};
    use crate::env;
    use crate::store::StorageAdaptor;

    /// A `FugueText` in an explicit storage scope with a deterministic
    /// collection id, so two scopes standing in for two replicas agree on
    /// entity ids for the same `field_name`.
    fn doc_in<S: StorageAdaptor>(field_name: &str) -> FugueText<S> {
        FugueText {
            blocks: UnorderedMap::new_with_field_name_and_crdt_type(
                None,
                field_name,
                crate::collections::CrdtType::FugueText,
            ),
        }
    }

    /// Stored blocks sorted by id — the canonical form for equality assertions.
    fn stored<S: StorageAdaptor>(doc: &FugueText<S>) -> Vec<(BlockId, TextBlock)> {
        let mut out: Vec<(BlockId, TextBlock)> = doc
            .blocks
            .entries()
            .unwrap()
            .map(|(k, v)| (k.id(), v))
            .collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    /// (1) Round trip, and the run is ONE entity, not five.
    #[test]
    fn insert_str__round_trips_as_a_single_block() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str(0, "hello").unwrap();
        assert_eq!(doc.get_text().unwrap(), "hello");
        assert_eq!(doc.len().unwrap(), 5);
        assert_eq!(
            doc.blocks.len().unwrap(),
            1,
            "a five-character run must be one entity"
        );
    }

    /// (2) Append coalescing — the whole point of blocks.
    #[test]
    fn insert__sequential_appends_extend_one_block() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert(0, 'a').unwrap();
        doc.insert(1, 'b').unwrap();
        doc.insert(2, 'c').unwrap();
        assert_eq!(doc.get_text().unwrap(), "abc");
        assert_eq!(
            doc.blocks.len().unwrap(),
            1,
            "three sequential appends must coalesce into one entity"
        );
    }

    /// (3) Mid-run insert splits the run by the deterministic rule.
    #[test]
    fn insert__mid_run_splits_deterministically() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str_with_replica(0, 7, "hello").unwrap();
        doc.insert_str_with_replica(2, 7, "X").unwrap();
        assert_eq!(doc.get_text().unwrap(), "heXllo");

        // (7,0)"hello" splits before offset 2 into (7,0)"he" and (7,2)"llo";
        // the tail's parent is the last node of the head, side R. 'X' is a new
        // one-character block hanging left off the tail's first node.
        let blocks = stored(&doc);
        assert_eq!(blocks.len(), 3, "expected head, tail and the inserted run");

        let head = &blocks[0];
        assert_eq!(head.0, BlockId::new(7, 0));
        assert_eq!(head.1.text, "he");
        assert_eq!(head.1.parent, None);
        assert_eq!(head.1.side, BlockSide::R);

        let tail = &blocks[1];
        assert_eq!(tail.0, BlockId::new(7, 2));
        assert_eq!(tail.1.text, "llo");
        assert_eq!(tail.1.parent, Some(BlockId::new(7, 1)));
        assert_eq!(tail.1.side, BlockSide::R);

        let inserted = &blocks[2];
        assert_eq!(inserted.0, BlockId::new(7, 5));
        assert_eq!(inserted.1.text, "X");
        assert_eq!(inserted.1.parent, Some(BlockId::new(7, 2)));
        assert_eq!(inserted.1.side, BlockSide::L);
    }

    /// (4) The same split on two independently built documents is byte-identical.
    #[test]
    fn insert__split_is_a_pure_function_of_block_and_offset() {
        type A = crate::store::MockedStorage<861>;
        type B = crate::store::MockedStorage<862>;
        env::reset_for_testing();

        let mut a = doc_in::<A>("split-a");
        a.insert_str_with_replica(0, 7, "hello").unwrap();
        a.insert_str_with_replica(2, 7, "X").unwrap();

        let mut b = doc_in::<B>("split-b");
        b.insert_str_with_replica(0, 7, "hello").unwrap();
        b.insert_str_with_replica(2, 7, "X").unwrap();

        assert_eq!(
            borsh::to_vec(&stored(&a)).unwrap(),
            borsh::to_vec(&stored(&b)).unwrap(),
            "split must be a pure function of (block, offset)"
        );
        assert_eq!(a.get_text().unwrap(), b.get_text().unwrap());
    }

    /// (5) A delete inside a run splits and tombstones; text and len agree.
    #[test]
    fn delete__inside_a_run_splits_and_tombstones() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str_with_replica(0, 7, "hello").unwrap();
        doc.delete(2).unwrap();

        assert_eq!(doc.get_text().unwrap(), "helo");
        assert_eq!(doc.len().unwrap(), 4);
        assert_eq!(doc.len().unwrap(), doc.get_text().unwrap().chars().count());

        let blocks = stored(&doc);
        assert_eq!(blocks.len(), 3, "the deleted region becomes its own block");
        assert_eq!(blocks[0].1.text, "he");
        assert!(!blocks[0].1.deleted);
        assert_eq!(blocks[1].0, BlockId::new(7, 2));
        assert_eq!(blocks[1].1.text, "l");
        assert!(
            blocks[1].1.deleted,
            "the deleted node must survive as a parent"
        );
        assert_eq!(blocks[2].0, BlockId::new(7, 3));
        assert_eq!(blocks[2].1.text, "lo");
        assert!(!blocks[2].1.deleted);
    }

    /// A replica seeded with the shared `S`, which appends `passage` and then
    /// goes back to insert `heading` immediately before its own passage.
    fn backward_insertion_doc<S: StorageAdaptor>(
        field_name: &str,
        replica: u64,
        heading: &str,
        passage: &str,
    ) -> FugueText<S> {
        let mut doc = doc_in::<S>(field_name);
        doc.insert_str_with_replica(0, 0, "S").unwrap();
        doc.insert_str_with_replica(1, replica, passage).unwrap();
        doc.insert_str_with_replica(1, replica, heading).unwrap();
        doc
    }

    /// (6) THE HEADLINE TEST, through the storage API: backward insertion does
    /// not interleave. This is the property RGA lacks.
    #[test]
    fn merge__backward_insertion_does_not_interleave() {
        type A = crate::store::MockedStorage<863>;
        type B = crate::store::MockedStorage<864>;
        env::reset_for_testing();

        let alice = backward_insertion_doc::<A>("doc", 1, "A", "aaa");
        let bob = backward_insertion_doc::<B>("doc", 2, "B", "bbb");
        assert_eq!(alice.get_text().unwrap(), "SAaaa");
        assert_eq!(bob.get_text().unwrap(), "SBbbb");

        let mut alice_then_bob = alice;
        alice_then_bob.merge_blocks_from(&bob).unwrap();
        let mut bob_then_alice = bob;
        bob_then_alice.merge_blocks_from(&alice_then_bob).unwrap();

        let merged = alice_then_bob.get_text().unwrap();
        assert_eq!(merged, bob_then_alice.get_text().unwrap());
        assert!(merged.contains("Aaaa"), "alice interleaved: {merged}");
        assert!(merged.contains("Bbbb"), "bob interleaved: {merged}");
        assert_eq!(merged, "SAaaaBbbb");
    }

    /// (7) Convergence: the same block set applied in any order reads the same.
    #[test]
    fn merge__converges_under_every_application_order() {
        type A = crate::store::MockedStorage<865>;
        type P = crate::store::MockedStorage<866>;
        env::reset_for_testing();

        let alice = backward_insertion_doc::<A>("conv", 1, "A", "aaa");
        let mut all = stored(&alice);
        // Two more concurrent replicas over the same seed.
        let bob = backward_insertion_doc::<crate::store::MockedStorage<867>>("conv", 2, "B", "bbb");
        let carol =
            backward_insertion_doc::<crate::store::MockedStorage<868>>("conv", 3, "C", "cc");
        all.extend(stored(&bob));
        all.extend(stored(&carol));
        all.sort_by_key(|(id, _)| *id);
        all.dedup_by_key(|(id, _)| *id);

        let mut reference: Option<String> = None;
        for round in 0..6_usize {
            let mut order = all.clone();
            match round {
                0 => {}
                1 => order.reverse(),
                n => order.rotate_left(n),
            }
            let mut doc = doc_in::<P>(&format!("perm{round}"));
            for (id, block) in order {
                let _ = doc.blocks.insert(BlockKey::new(id), block).unwrap();
            }
            let text = doc.get_text().unwrap();
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

    /// (8) Positions are char indices, not byte offsets.
    #[test]
    fn positions_are_char_indices_not_byte_offsets() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str(0, "héllo wörld").unwrap();
        doc.insert(5, '!').unwrap();
        assert_eq!(doc.get_text().unwrap(), "héllo! wörld");
    }

    /// (9a) The merge-mode guard fires for `insert`.
    #[test]
    #[should_panic(expected = "migration")]
    fn insert_panics_during_migration() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        env::with_merge_mode(|| {
            let _ = doc.insert(0, 'H');
        });
    }

    /// (9b) The merge-mode guard fires for `insert_str`.
    #[test]
    #[should_panic(expected = "migration")]
    fn insert_str_panics_during_migration() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        env::with_merge_mode(|| {
            let _ = doc.insert_str(0, "Hi");
        });
    }

    /// (9c) The deterministic replay API stays usable inside a migration.
    #[test]
    fn insert_str_with_replica_is_allowed_during_migration() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        env::with_merge_mode(|| {
            doc.insert_str_with_replica(0, 7, "H").unwrap();
        });
        assert_eq!(doc.len().unwrap(), 1);
    }
}
