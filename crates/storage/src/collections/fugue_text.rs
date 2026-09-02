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

use std::collections::BTreeMap;

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
    /// Tombstones, **one bit per node of the run**, LSB-first, trailing zero
    /// bytes trimmed so the encoding of a given tombstone set is unique.
    ///
    /// Fugue nodes must survive deletion — a tombstone can still be the parent
    /// of live nodes — so a delete can never drop the entity, and
    /// `UnorderedMap::remove` (RGA's mechanism) is unusable here.
    ///
    /// The bitmap is per **node**, not per run, and that is load-bearing rather
    /// than an optimisation. A run's `text` GROWS after the fact through
    /// coalescing, so a whole-run flag cannot say which nodes it covered: a
    /// short tombstoned copy merged with a longer live copy would tombstone
    /// nodes the deleter never saw. Bit-for-bit elementwise OR is the correct
    /// join, and it is what makes merge a monotone lattice operation.
    tombstones: Vec<u8>,
}

impl TextBlock {
    /// The number of nodes in this run.
    fn len(&self) -> usize {
        self.text.chars().count()
    }
}

/// Whether node `offset` of a run is tombstoned.
fn tomb_get(bits: &[u8], offset: usize) -> bool {
    bits.get(offset / 8)
        .is_some_and(|byte| byte & (1_u8 << (offset % 8)) != 0)
}

/// Tombstone node `offset`, growing the bitmap as needed.
fn tomb_set(bits: &mut Vec<u8>, offset: usize) {
    let byte = offset / 8;
    if bits.len() <= byte {
        bits.resize(byte + 1, 0);
    }
    if let Some(slot) = bits.get_mut(byte) {
        *slot |= 1_u8 << (offset % 8);
    }
}

/// Elementwise OR — the merge join for two copies of one run.
fn tomb_or(dst: &mut Vec<u8>, src: &[u8]) {
    if dst.len() < src.len() {
        dst.resize(src.len(), 0);
    }
    for (slot, byte) in dst.iter_mut().zip(src) {
        *slot |= *byte;
    }
    tomb_trim(dst);
}

/// The `from..to` sub-range of a bitmap, rebased to offset 0.
fn tomb_slice(bits: &[u8], from: usize, to: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for offset in from..to {
        if tomb_get(bits, offset) {
            tomb_set(&mut out, offset - from);
        }
    }
    out
}

/// Drop trailing zero bytes, so one tombstone set has exactly one encoding.
fn tomb_trim(bits: &mut Vec<u8>) {
    while bits.last() == Some(&0) {
        let _ignored = bits.pop();
    }
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
    /// Deleting does **not** split. Tombstones are per node, so a run is
    /// tombstoned in place — `delete_range(0, n)` over a typed run leaves that
    /// one entity, where a per-run flag would have had to shatter it.
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

        // Overlapping copies of one run can both define a node (a split that
        // raced a remote unsplit copy). Set the bit in EVERY copy: a tombstone
        // recorded in only one of them would be dropped by a later merge with a
        // peer that holds the other.
        let mut touched: BTreeMap<usize, TextBlock> = BTreeMap::new();
        for raw in targets {
            let id = BlockId::from_raw(raw);
            let mut covered = false;
            for index in blocks_containing(&loaded, id) {
                covered = true;
                let block = touched
                    .entry(index)
                    .or_insert_with(|| loaded[index].block.clone());
                tomb_set(
                    &mut block.tombstones,
                    (id.counter - loaded[index].id.counter) as usize,
                );
            }
            if !covered {
                return Err(invalid("deleted node has no block"));
            }
        }

        for (index, mut block) in touched {
            tomb_trim(&mut block.tombstones);
            let _ignored = self.blocks.insert(BlockKey::new(loaded[index].id), block)?;
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
                        self.write_segments(lb, &[0, offset, lb.len])?;
                    }
                }
                Side::R => {
                    if offset + 1 < lb.len {
                        // Fugue only picks side R when the parent has no right
                        // child, which a mid-run node always has — so this is
                        // defensive, not a path normal editing reaches.
                        self.write_segments(lb, &[0, offset + 1, lb.len])?;
                    } else if coalesces_into(lb, id) {
                        // The new node continues this very run: extend it
                        // instead of minting a second entity. This is what
                        // makes sequential typing cost one entity, not n. The
                        // appended node is live, and a trailing zero bit is
                        // implicit, so the bitmap is untouched.
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
            tombstones: Vec::new(),
        };
        let _ignored = self.blocks.insert(BlockKey::new(id), block)?;
        Ok(())
    }

    /// Rewrite `lb` as the segments delimited by `bounds` (which starts at 0 and
    /// ends at `lb.len`).
    ///
    /// The first segment keeps the original key, parent and side; every later
    /// segment starts a new entity whose parent is the last node of the segment
    /// before it, on side R — which is exactly the implicit intra-run edge, so
    /// the split changes the storage layout and nothing about the tree. Each
    /// segment carries the corresponding slice of the tombstone bitmap.
    fn write_segments(&mut self, lb: &LoadedBlock, bounds: &[usize]) -> Result<(), StoreError> {
        for segment in bounds.windows(2) {
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
                tombstones: tomb_slice(&lb.block.tombstones, from, to),
            };
            let _ignored = self.blocks.insert(BlockKey::new(id), block)?;
        }
        Ok(())
    }

    /// Every stored block, ascending by id, with its node count.
    fn load(&self) -> Result<Vec<LoadedBlock>, StoreError> {
        let mut loaded: Vec<LoadedBlock> = self
            .blocks
            .entries()?
            .map(|(key, block)| LoadedBlock {
                id: key.id(),
                len: block.len(),
                block,
            })
            .collect();
        loaded.sort_by_key(|lb| lb.id);
        Ok(loaded)
    }

    /// Copy every block from `other` that `self` neither holds nor tombstoned.
    ///
    /// The per-key join is a genuine lattice meet-free join, not an LWW
    /// heuristic: tombstone bitmaps are ORed elementwise, and the longer `text`
    /// wins. A run only ever grows at its tail, and only its own replica can
    /// grow it, so two copies of one key are always prefixes of one another —
    /// the longer one therefore contains the shorter, and taking it can never
    /// lose a node. A block `self` removed at the storage layer stays removed.
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
                tomb_or(&mut merged.tombstones, &incoming.tombstones);
                if incoming.len() > merged.len() {
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
    /// The run's node count.
    len: usize,
}

/// Whether the run `lb` can absorb a new node `id` as one more character.
///
/// Requires the node to continue the run's own id sequence, on the same replica.
/// `next_counter` mints strictly past every node the replica has defined, so the
/// absorbed id can never collide with an existing one.
fn coalesces_into(lb: &LoadedBlock, id: BlockId) -> bool {
    lb.id.replica == id.replica && u64::from(lb.id.counter) + lb.len as u64 == u64::from(id.counter)
}

/// Every block whose run covers `id`, ascending by block id.
///
/// More than one can: a split that raced a remote copy of the unsplit run
/// leaves overlapping definitions of the same nodes. They agree on value,
/// parent and side by construction, so overlap is harmless for reads — but a
/// tombstone must be written to all of them.
fn blocks_containing(loaded: &[LoadedBlock], id: BlockId) -> Vec<usize> {
    loaded
        .iter()
        .enumerate()
        .filter(|(_, lb)| {
            lb.id.replica == id.replica
                && lb.id.counter <= id.counter
                && u64::from(id.counter) < u64::from(lb.id.counter) + lb.len as u64
        })
        .map(|(index, _)| index)
        .collect()
}

/// The tightest block covering `id` — the one with the largest start id, i.e.
/// the most finely split definition. Deterministic, so two replicas that hold
/// the same block set make the same split and coalescing decisions.
fn find_block(loaded: &[LoadedBlock], id: BlockId) -> Option<usize> {
    blocks_containing(loaded, id).into_iter().next_back()
}

/// The next unused counter for `replica`, one past the highest node it minted.
///
/// Derived from the stored set rather than from node-local state: runs are
/// tombstoned in place and never removed (Fugue nodes must survive deletion),
/// so the high-water mark survives every deletion and a counter is never
/// reused. Any future garbage collection of tombstoned blocks would break that
/// and must carry the high-water mark some other way.
fn next_counter(replica: u64, loaded: &[LoadedBlock]) -> Result<u32, StoreError> {
    let mut next: u64 = 0;
    for lb in loaded {
        if lb.id.replica == replica {
            next = next.max(u64::from(lb.id.counter) + lb.len as u64);
        }
    }
    u32::try_from(next).map_err(|_| invalid("replica counter space exhausted"))
}

/// Expand the stored blocks into a Fugue tree.
///
/// Node `i` of a run is `(replica, counter + i)`; the first hangs off the
/// stored `(parent, side)`, every later one is the right child of its
/// predecessor. Tombstoned nodes are still emitted: they take part in the
/// traversal and can still parent live nodes.
///
/// Every block is expanded at its **full** length and duplicate node ids are
/// folded together by [`FugueTree::integrate`] — which is order-independent and
/// keeps the tombstone when a node is defined both live and dead. Overlapping
/// definitions of one node agree on value, parent and side by construction, so
/// this join is exact. (An earlier version truncated a run where the next run of
/// the same replica began, which silently dropped nodes whenever the shorter
/// run did not cover the whole remainder — coalescing makes that routine.)
fn build_tree(loaded: &[LoadedBlock]) -> Result<FugueTree, StoreError> {
    let mut tree = FugueTree::new();
    for lb in loaded {
        for (offset, content) in lb.block.text.chars().enumerate() {
            let offset_u32 = u32::try_from(offset).map_err(|_| invalid("run too long"))?;
            let counter = lb
                .id
                .counter
                .checked_add(offset_u32)
                .ok_or_else(|| invalid("node counter overflow"))?;
            let (parent, side) = if offset == 0 {
                (lb.block.parent.map(BlockId::raw), lb.block.side.into())
            } else {
                (Some((lb.id.replica, counter - 1)), Side::R)
            };
            tree.integrate(FugueNode {
                id: (lb.id.replica, counter),
                value: (!tomb_get(&lb.block.tombstones, offset)).then_some(content),
                parent,
                side,
            });
        }
    }
    Ok(tree)
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
    use super::{BlockId, BlockSide, FugueText, TextBlock};
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

    /// (5) A delete inside a run tombstones the node in place; text and len
    /// agree, and the run is NOT shattered.
    #[test]
    fn delete__inside_a_run_tombstones_without_splitting() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str_with_replica(0, 7, "hello").unwrap();
        doc.delete(2).unwrap();

        assert_eq!(doc.get_text().unwrap(), "helo");
        assert_eq!(doc.len().unwrap(), 4);
        assert_eq!(doc.len().unwrap(), doc.get_text().unwrap().chars().count());

        let blocks = stored(&doc);
        assert_eq!(
            blocks.len(),
            1,
            "per-node tombstones must not shatter the run"
        );
        assert_eq!(blocks[0].1.text, "hello", "the node survives as a parent");
        assert_eq!(blocks[0].1.tombstones, vec![0b0000_0100]);
    }

    /// (5b) Deleting a whole run leaves ONE tombstoned entity, not `n`.
    #[test]
    fn delete_range__whole_run_stays_one_block() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str_with_replica(0, 7, "hello").unwrap();
        doc.delete_range(0, 5).unwrap();

        assert_eq!(doc.get_text().unwrap(), "");
        assert!(doc.is_empty().unwrap());
        let blocks = stored(&doc);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].1.tombstones, vec![0b0001_1111]);
    }

    /// (5c) C1: a run that GREW by coalescing, merged with a shorter copy the
    /// peer tombstoned, must tombstone only the nodes the deleter actually saw.
    /// A per-run flag gets this wrong and silently eats the appended node.
    #[test]
    fn merge__tombstone_of_a_short_copy_does_not_eat_a_coalesced_node() {
        type A = crate::store::MockedStorage<869>;
        type B = crate::store::MockedStorage<870>;
        env::reset_for_testing();

        // Shared history: replica 2 typed "P".
        let mut a = doc_in::<A>("c1");
        a.insert_str_with_replica(0, 2, "P").unwrap();
        let mut b = doc_in::<B>("c1");
        b.insert_str_with_replica(0, 2, "P").unwrap();

        // B types on, coalescing "M" into the SAME entity; A deletes "P".
        b.insert_str_with_replica(1, 2, "M").unwrap();
        a.delete(0).unwrap();
        assert_eq!(b.get_text().unwrap(), "PM");
        assert_eq!(a.get_text().unwrap(), "");

        let mut a_then_b = doc_in::<A>("c1-ab");
        a_then_b.merge_blocks_from(&a).unwrap();
        a_then_b.merge_blocks_from(&b).unwrap();
        let mut b_then_a = doc_in::<B>("c1-ba");
        b_then_a.merge_blocks_from(&b).unwrap();
        b_then_a.merge_blocks_from(&a).unwrap();

        assert_eq!(a_then_b.get_text().unwrap(), "M");
        assert_eq!(b_then_a.get_text().unwrap(), "M");
    }

    /// (5d) C2: a run that one replica SPLIT and the other COALESCED must, when
    /// merged, leave no node uncovered. The old rule truncated a run where the
    /// next run of the same replica began, which assumed the two copies form an
    /// exact partition — coalescing breaks that, and the appended node vanished
    /// from every replica.
    #[test]
    fn merge__coalesced_run_meeting_a_split_copy_loses_no_node() {
        type A = crate::store::MockedStorage<871>;
        type B = crate::store::MockedStorage<872>;
        env::reset_for_testing();

        let mut a = doc_in::<A>("c2");
        a.insert_str_with_replica(0, 2, "ab").unwrap();
        let mut b = doc_in::<B>("c2");
        b.insert_str_with_replica(0, 2, "ab").unwrap();

        // B appends, coalescing (2,0) into "abc". A inserts mid-run, splitting
        // (2,0) into "a" + (2,1)"b". The merged set therefore holds (2,0)"abc"
        // overlapping (2,1)"b".
        b.insert_str_with_replica(2, 2, "c").unwrap();
        a.insert_str_with_replica(1, 1, "Q").unwrap();
        assert_eq!(b.get_text().unwrap(), "abc");
        assert_eq!(a.get_text().unwrap(), "aQb");

        let mut a_then_b = doc_in::<A>("c2-ab");
        a_then_b.merge_blocks_from(&a).unwrap();
        a_then_b.merge_blocks_from(&b).unwrap();
        let mut b_then_a = doc_in::<B>("c2-ba");
        b_then_a.merge_blocks_from(&b).unwrap();
        b_then_a.merge_blocks_from(&a).unwrap();

        assert_eq!(a_then_b.get_text().unwrap(), "aQbc");
        assert_eq!(b_then_a.get_text().unwrap(), "aQbc");
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

        // Commutativity, not idempotence: each order merges the two PRISTINE
        // replicas into a fresh document. Merging the already-merged copy would
        // only have asserted idempotence.
        let mut alice_then_bob = doc_in::<A>("doc-ab");
        alice_then_bob.merge_blocks_from(&alice).unwrap();
        alice_then_bob.merge_blocks_from(&bob).unwrap();
        let mut bob_then_alice = doc_in::<B>("doc-ba");
        bob_then_alice.merge_blocks_from(&bob).unwrap();
        bob_then_alice.merge_blocks_from(&alice).unwrap();

        let merged = alice_then_bob.get_text().unwrap();
        assert_eq!(merged, bob_then_alice.get_text().unwrap());
        assert!(merged.contains("Aaaa"), "alice interleaved: {merged}");
        assert!(merged.contains("Bbbb"), "bob interleaved: {merged}");
        assert_eq!(merged, "SAaaaBbbb");
    }

    /// (7) Convergence: three concurrent replicas merged in all six orders read
    /// the same.
    ///
    /// The order permuted is the **merge** order. Permuting `blocks.insert`
    /// calls would prove nothing: `load` sorts by id, so it discards any
    /// insertion order and the test could not fail.
    #[test]
    fn merge__converges_under_every_merge_order() {
        type A = crate::store::MockedStorage<865>;
        type B = crate::store::MockedStorage<866>;
        type C = crate::store::MockedStorage<867>;
        type P = crate::store::MockedStorage<868>;
        env::reset_for_testing();

        let alice = backward_insertion_doc::<A>("conv", 1, "A", "aaa");
        let bob = backward_insertion_doc::<B>("conv", 2, "B", "bbb");
        let carol = backward_insertion_doc::<C>("conv", 3, "C", "cc");

        let mut reference: Option<String> = None;
        for (round, order) in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ]
        .into_iter()
        .enumerate()
        {
            let mut doc = doc_in::<P>(&format!("perm{round}"));
            for which in order {
                match which {
                    0 => doc.merge_blocks_from(&alice).unwrap(),
                    1 => doc.merge_blocks_from(&bob).unwrap(),
                    _ => doc.merge_blocks_from(&carol).unwrap(),
                }
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

/// Differential and exhaustive verification against `fugue.rs` as the model.
///
/// `fugue.rs` already *is* Algorithm 1 of the paper, unaware of storage, blocks,
/// splitting, coalescing or merging. Running the same op script through both and
/// comparing text is therefore a genuine differential test of everything this
/// module adds, not a restatement of it.
///
/// Every script mixes the three interactions that matter, because it is their
/// COMBINATION — coalescing against splitting against deleting — that hid the
/// two data-loss bugs the first implementation shipped.
#[cfg(test)]
mod model_tests {
    use std::collections::BTreeMap;

    use super::{FugueText, TextBlock, UnorderedMap};
    use crate::collections::fugue::{FugueNode, FugueTree, Rng};
    use crate::env;
    use crate::store::{MockedStorage, StorageAdaptor};

    /// The oracle: Algorithm 1, with the same id allocation `FugueText` uses.
    ///
    /// A replica's counter is simply the number of nodes it has minted, which is
    /// exactly what `next_counter` derives from the block set — runs of one
    /// replica always cover `0..N` contiguously, whether split, coalesced or
    /// tombstoned.
    #[derive(Clone, Debug, Default)]
    struct Model {
        tree: FugueTree,
        next: BTreeMap<u64, u32>,
    }

    impl Model {
        fn insert_str(&mut self, pos: usize, replica: u64, s: &str) {
            for (offset, content) in s.chars().enumerate() {
                let counter = self.next.entry(replica).or_default();
                let id = (replica, *counter);
                *counter += 1;
                let _ignored = self.tree.insert(pos + offset, content, id).unwrap();
            }
        }

        fn delete_range(&mut self, start: usize, end: usize) {
            let count = end.min(self.tree.len()).saturating_sub(start);
            for _ in 0..count {
                if self.tree.delete(start).is_err() {
                    break;
                }
            }
        }

        fn text(&self) -> String {
            self.tree.values()
        }

        fn nodes(&self) -> Vec<FugueNode> {
            self.tree.nodes().copied().collect()
        }

        /// The union of several models: every node, delete-wins per node, read
        /// through a fresh tree. `integrate` is order-independent and keeps a
        /// tombstone over a live copy, so this is exactly the CRDT join.
        fn union(models: &[&Self]) -> String {
            let mut tree = FugueTree::new();
            for model in models {
                for node in model.nodes() {
                    tree.integrate(node);
                }
            }
            for model in models {
                for node in model.nodes() {
                    if node.value.is_none() {
                        tree.integrate(node);
                    }
                }
            }
            tree.values()
        }
    }

    /// One edit. `Ins` appends when `pos` lands at the end, which is what makes
    /// runs coalesce; a `pos` in the middle is what makes them split.
    #[derive(Clone, Debug)]
    enum Op {
        Ins(usize, &'static str),
        Del(usize, usize),
    }

    fn doc_in<S: StorageAdaptor>(field_name: &str) -> FugueText<S> {
        FugueText {
            blocks: UnorderedMap::new_with_field_name_and_crdt_type(
                None,
                field_name,
                crate::collections::CrdtType::FugueText,
            ),
        }
    }

    /// Apply one op to the document and to the oracle, asserting they agree.
    fn apply<S: StorageAdaptor>(doc: &mut FugueText<S>, model: &mut Model, replica: u64, op: &Op) {
        let len = model.text().chars().count();
        match *op {
            Op::Ins(pos, text) => {
                let pos = pos % (len + 1);
                doc.insert_str_with_replica(pos, replica, text).unwrap();
                model.insert_str(pos, replica, text);
            }
            Op::Del(start, span) => {
                let start = start % (len + 1);
                let end = start + span;
                doc.delete_range(start, end).unwrap();
                model.delete_range(start, end);
            }
        }
        assert_eq!(
            doc.get_text().unwrap(),
            model.text(),
            "local state diverged from the model after {op:?}"
        );
    }

    /// Seed both replicas with an identical, already-synchronised history.
    fn seed<S: StorageAdaptor>(doc: &mut FugueText<S>, model: &mut Model) {
        doc.insert_str_with_replica(0, 0, "seed").unwrap();
        model.insert_str(0, 0, "seed");
    }

    /// Run one two-replica scenario: edit concurrently, then merge in BOTH
    /// orders from the pristine states, and check every result against the
    /// model.
    fn scenario(tag: &str, left: &[Op], right: &[Op]) {
        type S = MockedStorage<873>;

        let mut a = doc_in::<S>(&format!("{tag}-a"));
        let mut b = doc_in::<S>(&format!("{tag}-b"));
        let (mut ma, mut mb) = (Model::default(), Model::default());
        seed(&mut a, &mut ma);
        seed(&mut b, &mut mb);

        for op in left {
            apply(&mut a, &mut ma, 1, op);
        }
        for op in right {
            apply(&mut b, &mut mb, 2, op);
        }

        let expected = Model::union(&[&ma, &mb]);

        let mut ab = doc_in::<S>(&format!("{tag}-ab"));
        ab.merge_blocks_from(&a).unwrap();
        ab.merge_blocks_from(&b).unwrap();
        let mut ba = doc_in::<S>(&format!("{tag}-ba"));
        ba.merge_blocks_from(&b).unwrap();
        ba.merge_blocks_from(&a).unwrap();

        assert_eq!(
            ab.get_text().unwrap(),
            expected,
            "{tag}: A-then-B disagrees with the model (left={left:?} right={right:?})"
        );
        assert_eq!(
            ba.get_text().unwrap(),
            expected,
            "{tag}: B-then-A disagrees with the model (left={left:?} right={right:?})"
        );
    }

    /// The op alphabet used by the exhaustive sweep: one appending insert (which
    /// coalesces), two mid-document inserts (which split), and two deletes (one
    /// single, one spanning).
    const ALPHABET: [Op; 5] = [
        Op::Ins(usize::MAX, "x"),
        Op::Ins(1, "y"),
        Op::Ins(2, "zz"),
        Op::Del(1, 1),
        Op::Del(0, 3),
    ];

    /// EXHAUSTIVE, not sampled: every pair of ops on each of two replicas —
    /// 5^2 x 5^2 = 625 scripts, each merged in both orders and checked against
    /// the model. This is the bounded-interleaving proof for k = 2, n = 2.
    #[test]
    fn exhaustive__all_two_op_interleavings_across_two_replicas() {
        env::reset_for_testing();
        let mut checked = 0_usize;
        for (a0, first_left) in ALPHABET.iter().enumerate() {
            for (a1, second_left) in ALPHABET.iter().enumerate() {
                for (b0, first_right) in ALPHABET.iter().enumerate() {
                    for (b1, second_right) in ALPHABET.iter().enumerate() {
                        let left = [first_left.clone(), second_left.clone()];
                        let right = [first_right.clone(), second_right.clone()];
                        scenario(&format!("ex{a0}{a1}{b0}{b1}"), &left, &right);
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 625, "the sweep must be exhaustive, not sampled");
    }

    /// 200-seed differential fuzz against the model, with random multi-op
    /// scripts on two replicas and both merge orders per seed.
    #[test]
    fn fuzz__two_hundred_seeds_match_the_model() {
        env::reset_for_testing();
        const WORDS: [&str; 4] = ["a", "bc", "d", "ef"];

        let mut rng = Rng::new(0x_f0_5e_ed_01);
        for seed in 0..200_usize {
            let script = |rng: &mut Rng| -> Vec<Op> {
                let count = 1 + rng.below(4);
                (0..count)
                    .map(|_| {
                        if rng.below(3) == 0 {
                            Op::Del(rng.below(8), 1 + rng.below(3))
                        } else {
                            Op::Ins(rng.below(8), WORDS[rng.below(WORDS.len())])
                        }
                    })
                    .collect()
            };
            let left = script(&mut rng);
            let right = script(&mut rng);
            scenario(&format!("fz{seed}"), &left, &right);
        }
    }

    /// The bitmap encoding is canonical: one tombstone set, one byte string.
    /// Merge relies on it — `merged != mine` decides whether to write.
    #[test]
    fn tombstone_encoding_is_canonical() {
        env::reset_for_testing();
        type S = MockedStorage<874>;
        let mut doc = doc_in::<S>("canon");
        doc.insert_str_with_replica(0, 1, "abcdefghij").unwrap();
        doc.delete_range(0, 10).unwrap();
        doc.insert_str_with_replica(0, 1, "k").unwrap();

        let blocks: Vec<TextBlock> = doc.blocks.entries().unwrap().map(|(_, v)| v).collect();
        for block in &blocks {
            assert_ne!(
                block.tombstones.last(),
                Some(&0),
                "trailing zero bytes must be trimmed"
            );
        }
        assert_eq!(doc.get_text().unwrap(), "k");
    }
}
