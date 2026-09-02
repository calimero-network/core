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

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};

use super::fugue::{FugueNode, FugueTree, Side};
use super::{CrdtType, UnorderedMap};
use crate::collections::error::StoreError;
use crate::env;
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
    /// Panics if called inside a state migration (`#[app::migrate]`, i.e.
    /// storage merge mode): node ids are minted from the node-local device id,
    /// so a migrate body would mint a different id on every node and diverge
    /// the network. Carry the document across unchanged, or seed with
    /// [`insert_str_with_replica`](Self::insert_str_with_replica).
    ///
    /// # Errors
    /// Returns an error if `pos` is out of bounds or storage fails.
    #[expect(
        clippy::panic,
        reason = "non-deterministic during migrate (node-local device id); a loud panic is \
                  the intended, unmissable guard against a silent network divergence"
    )]
    pub fn insert(&mut self, pos: usize, content: char) -> Result<(), StoreError> {
        if env::in_merge_mode() {
            panic!(
                "FugueText::insert() is non-deterministic during a state migration: it \
                 mints node ids from the node-local device id, producing a different id \
                 per node and diverging the network. Carry the document across unchanged \
                 or seed with `insert_str_with_replica(pos, replica, s)`."
            );
        }
        let replica = local_replica();
        self.insert_str_with_replica(pos, replica, content.encode_utf8(&mut [0_u8; 4]))
    }

    /// Insert a string at the given visible position.
    ///
    /// # Panics
    /// Panics if called inside a state migration — see [`insert`](Self::insert).
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
                "FugueText::insert_str() is non-deterministic during a state migration: it \
                 mints node ids from the node-local device id, diverging ids across nodes. \
                 Seed with `insert_str_with_replica(pos, replica, s)`."
            );
        }
        let replica = local_replica();
        self.insert_str_with_replica(pos, replica, s)
    }

    /// Insert a string at `pos`, minting node ids under an explicit `replica`.
    ///
    /// This is the deterministic counterpart of [`insert_str`](Self::insert_str)
    /// — the analogue of RGA's `insert_str_at_timestamp`. Fugue ids are
    /// `(replica, counter)` rather than `(hlc, seq)`, and the counter is derived
    /// from the stored block set (never from a clock), so fixing the replica
    /// makes the whole resulting state a pure function of the inputs. That is
    /// what the structural-equality laws in `tests/crdt_contract.rs` need, and
    /// it is the sanctioned way to seed a document inside a migration.
    ///
    /// The replica must be genuinely unique per writer: two writers sharing one
    /// replica id mint colliding node ids and diverge. Production callers should
    /// prefer [`insert_str`](Self::insert_str), which derives it from the device.
    ///
    /// # Errors
    /// Returns an error if `pos` is out of bounds or storage fails.
    pub fn insert_str_with_replica(
        &mut self,
        pos: usize,
        replica: u64,
        s: &str,
    ) -> Result<(), StoreError> {
        // Register this type's nested-id re-key thunk so a document stored as a
        // collection value is re-keyed when the outer collection is stored.
        let _ignored = super::rekey::register_rekey::<Self>();

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
    /// Idempotent in `end`: a range past the end of the document deletes up to
    /// the end rather than erroring, so a delete replayed out of order is safe.
    ///
    /// # Errors
    /// Returns an error if `start > end` or storage fails.
    pub fn delete_range(&mut self, start: usize, end: usize) -> Result<(), StoreError> {
        if start > end {
            return Err(invalid("start must be <= end"));
        }

        let loaded = self.load()?;
        let mut tree = build_tree(&loaded)?;

        // `FugueTree::delete` returns the id it tombstoned, which is the only
        // way to get document-order ids out of the pure module. Deleting at
        // `start` repeatedly walks forward, because each tombstone removes that
        // character from the live sequence.
        let count = end.min(tree.len()).saturating_sub(start);
        let mut targets: Vec<(u64, u32)> = Vec::with_capacity(count);
        for _ in 0..count {
            match tree.delete(start) {
                Ok(id) => targets.push(id),
                Err(_) => break,
            }
        }
        if targets.is_empty() {
            return Ok(());
        }

        // Group the doomed nodes by the block that holds them, as offsets
        // within that block.
        let mut by_block: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for raw in targets {
            let id = BlockId::from_raw(raw);
            let index =
                find_block(&loaded, id).ok_or_else(|| invalid("deleted node has no block"))?;
            let offset = (id.counter - loaded[index].id.counter) as usize;
            let _ignored = by_block.entry(index).or_default().insert(offset);
        }

        for (index, offsets) in by_block {
            let lb = &loaded[index];
            // Cut at every deletion boundary, so each resulting segment is
            // wholly deleted or wholly live.
            let mut cuts: BTreeSet<usize> = BTreeSet::new();
            for offset in &offsets {
                let _ignored = cuts.insert(*offset);
                let _ignored = cuts.insert(offset + 1);
            }
            let bounds = bounds_from_cuts(lb.len, &cuts);
            let flags: Vec<bool> = bounds
                .windows(2)
                .map(|w| lb.block.deleted || offsets.contains(&w[0]))
                .collect();
            self.write_segments(lb, &bounds, &flags)?;
        }

        Ok(())
    }

    /// The document text, tombstones excluded.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn get_text(&self) -> Result<String, StoreError> {
        let loaded = self.load()?;
        Ok(build_tree(&loaded)?.values())
    }

    /// The number of visible characters.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn len(&self) -> Result<usize, StoreError> {
        let loaded = self.load()?;
        Ok(build_tree(&loaded)?.len())
    }

    /// Whether the document has no visible characters.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        self.len().map(|len| len == 0)
    }

    /// Insert one character, minting `(replica, next_counter)`.
    fn insert_one(&mut self, pos: usize, replica: u64, content: char) -> Result<(), StoreError> {
        let loaded = self.load()?;
        let mut tree = build_tree(&loaded)?;
        let counter = next_counter(replica, &loaded)?;

        let node = tree
            .insert(pos, content, (replica, counter))
            .map_err(|err| invalid(&err.to_string()))?;

        self.materialize(&loaded, node)
    }

    /// Write the node the tree just minted into the block store, splitting or
    /// extending an existing run as the rules below require.
    fn materialize(&mut self, loaded: &[LoadedBlock], node: FugueNode) -> Result<(), StoreError> {
        let id = BlockId::from_raw(node.id);
        let parent = node.parent.map(BlockId::from_raw);

        if let Some(parent_id) = parent {
            let index = find_block(loaded, parent_id)
                .ok_or_else(|| invalid("new node's parent is not in any stored block"))?;
            let lb = &loaded[index];
            let offset = (parent_id.counter - lb.id.counter) as usize;

            match node.side {
                // A left child of a mid-run node makes that node a run
                // boundary: split the run immediately BEFORE it. This is the
                // deterministic split rule — a pure function of (block, offset).
                Side::L => {
                    if offset > 0 {
                        let bounds = [0, offset, lb.len];
                        let flags = [lb.block.deleted, lb.block.deleted];
                        self.write_segments(lb, &bounds, &flags)?;
                    }
                }
                Side::R => {
                    if offset + 1 < lb.len {
                        // Fugue only picks side R when the parent has no right
                        // child, which a mid-run node always has — so this is
                        // defensive, not a path normal editing reaches.
                        let bounds = [0, offset + 1, lb.len];
                        let flags = [lb.block.deleted, lb.block.deleted];
                        self.write_segments(lb, &bounds, &flags)?;
                    } else if coalesces_into(lb, id) {
                        // The new node continues this very run: extend it
                        // instead of minting a second entity. This is what
                        // makes sequential typing cost one entity, not n.
                        let mut block = lb.block.clone();
                        block.text.push(node.value.unwrap_or('\u{fffd}'));
                        let _ignored = self.blocks.insert(BlockKey::new(lb.id), block)?;
                        return Ok(());
                    }
                }
            }
        }

        let block = TextBlock {
            start_id: id,
            text: node.value.unwrap_or('\u{fffd}').to_string(),
            parent,
            side: node.side.into(),
            deleted: false,
        };
        let _ignored = self.blocks.insert(BlockKey::new(id), block)?;
        Ok(())
    }

    /// Rewrite `lb` as the segments delimited by `bounds` (which starts at 0 and
    /// ends at `lb.len`), each flagged live or deleted by `flags`.
    ///
    /// The first segment keeps the original key, parent and side; every later
    /// segment starts a new entity whose parent is the last node of the segment
    /// before it, on side R — which is exactly the implicit intra-run edge, so
    /// the split changes the storage layout and nothing about the tree.
    fn write_segments(
        &mut self,
        lb: &LoadedBlock,
        bounds: &[usize],
        flags: &[bool],
    ) -> Result<(), StoreError> {
        for (segment, flag) in bounds.windows(2).zip(flags) {
            let (from, to) = (segment[0], segment[1]);
            let id = BlockId::new(
                lb.id.replica,
                lb.id
                    .counter
                    .checked_add(u32::try_from(from).map_err(|_| invalid("run too long"))?)
                    .ok_or_else(|| invalid("node counter overflow"))?,
            );
            let (parent, side) = if from == 0 {
                (lb.block.parent, lb.block.side)
            } else {
                (
                    Some(BlockId::new(lb.id.replica, id.counter - 1)),
                    BlockSide::R,
                )
            };
            let block = TextBlock {
                start_id: id,
                text: slice_chars(&lb.block.text, from, to).to_owned(),
                parent,
                side,
                deleted: *flag,
            };
            let _ignored = self.blocks.insert(BlockKey::new(id), block)?;
        }
        Ok(())
    }

    /// Every stored block, ascending by id, with its effective length.
    fn load(&self) -> Result<Vec<LoadedBlock>, StoreError> {
        let mut raw: Vec<(BlockId, TextBlock)> = self
            .blocks
            .entries()?
            .map(|(key, block)| (key.id(), block))
            .collect();
        raw.sort_by_key(|(id, _)| *id);

        let mut loaded: Vec<LoadedBlock> = Vec::with_capacity(raw.len());
        for (position, (id, block)) in raw.iter().enumerate() {
            let stored_len = block.text.chars().count();
            // Two blocks of one replica can overlap when a split raced a remote
            // copy of the unsplit run. Truncating a run at the start of the next
            // run of the same replica is a pure function of the block set, so
            // both replicas resolve the overlap identically.
            let mut len = stored_len;
            if let Some((next_id, _)) = raw.get(position + 1) {
                if next_id.replica == id.replica {
                    let gap = (next_id.counter - id.counter) as usize;
                    len = len.min(gap);
                }
            }
            loaded.push(LoadedBlock {
                id: *id,
                block: block.clone(),
                len,
                stored_len,
            });
        }
        Ok(loaded)
    }

    /// Copy every block from `other` that `self` neither holds nor tombstoned.
    ///
    /// Add-wins for unseen blocks, delete-wins for the per-block tombstone flag
    /// and for a block `self` removed at the storage layer. Where both hold the
    /// same key with different lengths (one side split the run, the other did
    /// not), the longer text is kept: `load` truncates the overlap on read, so
    /// keeping the longer copy can never lose a character.
    ///
    /// Generic over `S2` so the cross-store merge is testable.
    pub(crate) fn merge_blocks_from<S2: StorageAdaptor>(
        &mut self,
        other: &FugueText<S2>,
    ) -> Result<(), StoreError> {
        for (key, incoming) in other.blocks.entries()? {
            // Propagate a read error rather than treating it as "absent" — that
            // would re-insert over live data.
            if let Some(mine) = self.blocks.get(&key)? {
                let mine = mine.into_inner();
                let mut merged = mine.clone();
                merged.deleted = mine.deleted || incoming.deleted;
                if incoming.text.chars().count() > merged.text.chars().count() {
                    merged.text = incoming.text;
                }
                if merged != mine {
                    let _ignored = self.blocks.insert(key, merged)?;
                }
                continue;
            }

            // Absent from `self`'s live set: a tombstone means `self` deleted
            // this entity (delete wins, don't resurrect); otherwise it is new.
            if crate::index::Index::<S>::is_deleted(self.blocks.entry_id(&key))? {
                continue;
            }
            let _ignored = self.blocks.insert(key, incoming)?;
        }
        Ok(())
    }
}

/// A stored block plus the read-time facts derived from its neighbours.
struct LoadedBlock {
    /// The id of the run's first node.
    id: BlockId,
    /// The stored entity.
    block: TextBlock,
    /// The run's effective length: `stored_len`, truncated where a later run of
    /// the same replica starts inside it.
    len: usize,
    /// The run's stored length in characters.
    stored_len: usize,
}

/// Whether the run `lb` can absorb a new node `id` as one more character.
///
/// Requires the node to continue the run's own id sequence, on the same replica,
/// in a live and untruncated run.
fn coalesces_into(lb: &LoadedBlock, id: BlockId) -> bool {
    !lb.block.deleted
        && lb.len == lb.stored_len
        && lb.id.replica == id.replica
        && u64::from(lb.id.counter) + lb.len as u64 == u64::from(id.counter)
}

/// The index of the block whose run covers `id`, if any.
fn find_block(loaded: &[LoadedBlock], id: BlockId) -> Option<usize> {
    // `loaded` is sorted by id, so the candidate is the last block that starts
    // at or before `id`.
    let position = loaded.partition_point(|lb| lb.id <= id).checked_sub(1)?;
    let lb = &loaded[position];
    (lb.id.replica == id.replica
        && u64::from(id.counter) < u64::from(lb.id.counter) + lb.len as u64)
        .then_some(position)
}

/// The next unused counter for `replica`, one past the highest node it minted.
///
/// Derived from the stored set rather than from node-local state: deleted runs
/// stay in the map (Fugue tombstones in place), so the high-water mark survives
/// every deletion and a counter is never reused.
fn next_counter(replica: u64, loaded: &[LoadedBlock]) -> Result<u32, StoreError> {
    let mut next: u64 = 0;
    for lb in loaded {
        if lb.id.replica == replica {
            next = next.max(u64::from(lb.id.counter) + lb.stored_len as u64);
        }
    }
    u32::try_from(next).map_err(|_| invalid("replica counter space exhausted"))
}

/// Expand the stored blocks into a Fugue tree.
///
/// Node `i` of a run is `(replica, counter + i)`; the first hangs off the
/// stored `(parent, side)`, every later one is the right child of its
/// predecessor. A deleted run contributes tombstoned nodes, which still take
/// part in the traversal and can still parent live nodes.
fn build_tree(loaded: &[LoadedBlock]) -> Result<FugueTree, StoreError> {
    let mut tree = FugueTree::new();
    for lb in loaded {
        for (offset, content) in lb.block.text.chars().take(lb.len).enumerate() {
            let offset = u32::try_from(offset).map_err(|_| invalid("run too long"))?;
            let counter = lb
                .id
                .counter
                .checked_add(offset)
                .ok_or_else(|| invalid("node counter overflow"))?;
            let (parent, side) = if offset == 0 {
                (lb.block.parent.map(BlockId::raw), lb.block.side.into())
            } else {
                (Some((lb.id.replica, counter - 1)), Side::R)
            };
            tree.integrate(FugueNode {
                id: (lb.id.replica, counter),
                value: (!lb.block.deleted).then_some(content),
                parent,
                side,
            });
        }
    }
    Ok(tree)
}

/// `[0, ..cuts.., len]` with cuts outside `(0, len)` dropped.
fn bounds_from_cuts(len: usize, cuts: &BTreeSet<usize>) -> Vec<usize> {
    let mut bounds = Vec::with_capacity(cuts.len() + 2);
    bounds.push(0);
    bounds.extend(cuts.iter().copied().filter(|cut| *cut > 0 && *cut < len));
    bounds.push(len);
    bounds
}

/// The `from..to` character range of `text` (character indices, not bytes).
fn slice_chars(text: &str, from: usize, to: usize) -> &str {
    let byte_of = |offset: usize| {
        text.char_indices()
            .nth(offset)
            .map_or(text.len(), |(index, _)| index)
    };
    &text[byte_of(from)..byte_of(to)]
}

/// This node's replica id: the first 8 bytes of its device id.
///
/// A device is the writer identity here (an id is a *stamp*, not a gate), so two
/// devices of one account never share a counter space.
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
