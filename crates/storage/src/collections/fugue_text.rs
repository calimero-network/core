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
//!
//! ## The ordered index
//!
//! On top of that authoritative path sits a **node-local, derived, non-synced
//! ordered index** over the [`StorageAdaptor`] `index_*` seam — the same seam
//! [`SortedMap`](super::SortedMap) uses, with the same `full_hash` validity
//! marker. One index entry describes one **segment**: a maximal run of
//! consecutive nodes of one stored block that are also consecutive in document
//! order. The order key is the segment's live start position, so a range scan
//! over the index yields document order, and the key also carries the segment's
//! live length — which is what makes [`len`](FugueText::len) cost zero entity
//! loads and [`char_at`](FugueText::char_at) cost exactly one.
//!
//! Two properties of this index are load-bearing and must not be relaxed.
//!
//! **The index never feeds a write.** `parent` and `side` are *synced* fields.
//! An id resolved through a stale index does not produce a wrong local view
//! that a rebuild repairs — it mints a wrong causal edge that ships in the
//! delta and diverges every replica permanently. So
//! [`insert`](FugueText::insert), [`delete`](FugueText::delete) and
//! [`delete_range`](FugueText::delete_range) resolve positions against the
//! authoritative block set and the tree built from it, exactly as they did
//! before the index existed. The index is written by those paths, never read
//! by them. Reads that *do* consult it re-validate every entry against the
//! authoritative child set (see [`FugueText::indexed_segments`]) and fall back
//! to the tree on any disagreement.
//!
//! **Maintenance cost never depends on node-local state.** The index is
//! rebuilt *unconditionally* at the end of every mutating call — never "only if
//! the marker says it is stale". A marker-conditional rebuild would make the
//! same logical write cost different gas on a cold and a warm node, so one
//! replica could complete a write while another traps on `GasExhausted`; that
//! is divergence, and `crates/runtime/tests/metering_determinism.rs` exists to
//! catch it. Run-length blocks are what make the unconditional choice
//! affordable: the rebuild is `O(segments)`, and segments count *runs*, not
//! characters, on a path that already loads every block and builds the tree.
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
        // Unconditional, once per call: see the module doc on why a
        // marker-conditional rebuild would make write gas node-dependent.
        self.rebuild_index()
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
        // NOTE (Constraint 1): the ordered index is deliberately NOT consulted
        // here. `parent` and `side` are synced fields, so a position resolved
        // through a stale index would mint a wrong causal edge that ships in
        // the delta and diverges every replica permanently. `tree` above is
        // built from the authoritative block set.
        let count = end.min(tree.len()).saturating_sub(start);
        let mut targets: Vec<(u64, u32)> = Vec::with_capacity(count);
        for _ in 0..count {
            match tree.delete(start) {
                Ok(id) => targets.push(id),
                Err(_) => break,
            }
        }
        if targets.is_empty() {
            return self.rebuild_index();
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

        self.rebuild_index()
    }

    /// The document text, tombstones excluded.
    ///
    /// Served from the ordered index when it is valid (`O(segments)` block
    /// loads, no tree build), otherwise from the authoritative block set.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn get_text(&self) -> Result<String, StoreError> {
        if let Some(text) = self.indexed_text(0, usize::MAX)? {
            return Ok(text);
        }
        let loaded = self.load()?;
        Ok(build_tree(&loaded)?.values())
    }

    /// The characters in the half-open range `start..end`.
    ///
    /// Positions are `char` indices — Unicode scalar values, not bytes and not
    /// grapheme clusters.
    ///
    /// Clamped in `end` exactly like [`delete_range`](Self::delete_range): a
    /// range reaching past the end of the document returns up to the end rather
    /// than erroring. A reader that errored when a concurrent remote delete
    /// shrank the document out from under it would be a guaranteed production
    /// bug.
    ///
    /// # Errors
    /// Returns an error if `start > end` or storage fails.
    pub fn text_range(&self, start: usize, end: usize) -> Result<String, StoreError> {
        if start > end {
            return Err(invalid("start must be <= end"));
        }
        if let Some(text) = self.indexed_text(start, end)? {
            return Ok(text);
        }
        let loaded = self.load()?;
        let text = build_tree(&loaded)?.values();
        Ok(slice_chars(&text, start, end).to_owned())
    }

    /// The character at `pos`, or `None` if `pos` is past the end.
    ///
    /// Positions are `char` indices — Unicode scalar values, not bytes and not
    /// grapheme clusters.
    ///
    /// Served from the ordered index when it is valid, in which case exactly
    /// one block is loaded whatever the document's size.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn char_at(&self, pos: usize) -> Result<Option<char>, StoreError> {
        if let Some(text) = self.indexed_text(pos, pos.saturating_add(1))? {
            return Ok(text.chars().next());
        }
        let loaded = self.load()?;
        Ok(build_tree(&loaded)?.values().chars().nth(pos))
    }

    /// The number of visible characters.
    ///
    /// Served from the ordered index when it is valid, in which case no block
    /// is loaded at all — the live counts live in the order keys.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn len(&self) -> Result<usize, StoreError> {
        if let Some(segments) = self.indexed_segments()? {
            return Ok(segments
                .last()
                .map_or(0, |last| last.live_start + last.live_len));
        }
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

    /// The id of the collection the index is keyed under.
    fn collection_id(&self) -> crate::address::Id {
        use crate::entities::Data as _;
        self.blocks.element().id()
    }

    /// The collection's current `full_hash` — the ordered index's validity
    /// signal — or `None` if it cannot be read.
    ///
    /// Fails **closed**: a read error yields `None`, which can only ever mean
    /// "do not certify" and "do not trust". (`SortedMap::current_full_hash`
    /// substitutes `[0u8; 32]` on a read error and will happily stamp that as
    /// valid; that shape must not be copied.)
    fn current_full_hash(&self) -> Option<[u8; 32]> {
        match crate::index::Index::<S>::get_hashes_for(self.collection_id()) {
            Ok(Some((full, _own))) => Some(full),
            Ok(None) | Err(_) => None,
        }
    }

    /// `true` only if the stamped marker provably equals the collection's
    /// current `full_hash`. Unreadable hash or absent marker ⇒ `false`.
    fn index_marker_current(&self) -> bool {
        let Some(full) = self.current_full_hash() else {
            return false;
        };
        S::index_meta_get(self.collection_id()).as_deref() == Some(&full[..])
    }

    /// Rebuild the ordered index from the authoritative block set.
    ///
    /// Called **unconditionally** at the end of every mutating operation — see
    /// the module doc: a marker-conditional rebuild would make write gas depend
    /// on node-local state, which is cross-replica divergence, not slowness.
    /// Cost is `O(segments)` on a path that already loads every block.
    ///
    /// Normalises overlap as it goes: where two stored copies of one run
    /// disagree about a tombstone, the authoritative join (delete-wins, what
    /// [`build_tree`] computes) is written back into the owning copy, so the
    /// index's live counts and a later block-local read of the same window can
    /// never disagree.
    fn rebuild_index(&mut self) -> Result<(), StoreError> {
        if !S::index_supported() {
            return Ok(());
        }

        let loaded = self.load()?;
        let tree = build_tree(&loaded)?;
        let (segments, repairs) = segment_document(&loaded, &tree)?;

        for (index, block) in repairs {
            let _ignored = self.blocks.insert(BlockKey::new(loaded[index].id), block)?;
        }

        let collection = self.collection_id();
        let mut persisted = S::index_clear(collection);
        for segment in &segments {
            let entry = self.blocks.entry_id(&BlockKey::new(segment.block));
            persisted &= S::index_put(collection, &segment.order_key(), entry);
        }

        // Only stamp a marker we can prove: a dropped index write or an
        // unreadable hash leaves the marker stale, and the next read falls back
        // to the authoritative path instead of trusting a partial index.
        if persisted {
            if let Some(full) = self.current_full_hash() {
                let _ignored = S::index_meta_put(collection, &full);
            }
        }
        Ok(())
    }

    /// The index's segments in document order, or `None` when the index cannot
    /// be trusted for this read — the adaptor does not back it, the validity
    /// marker is stale, or the entries disagree with the authoritative child
    /// set.
    ///
    /// The validation is the point. Entries must form a dense `seq` sequence
    /// whose live lengths accumulate to the recorded start positions (which
    /// catches a dropped or duplicated entry), and the distinct blocks they
    /// name must number exactly as many as the collection actually holds
    /// (which catches a wiped or over-full index). Every block owns at least
    /// its own first node — no tighter block can start later than that — so
    /// that cardinality is an equality, not a bound.
    fn indexed_segments(&self) -> Result<Option<Vec<Segment>>, StoreError> {
        if !S::index_supported() || !self.index_marker_current() {
            return Ok(None);
        }

        let hits = S::index_range(
            self.collection_id(),
            core::ops::Bound::Unbounded,
            core::ops::Bound::Unbounded,
            0,
            None,
        );

        let mut segments = Vec::with_capacity(hits.len());
        let mut blocks: std::collections::BTreeSet<BlockId> = std::collections::BTreeSet::new();
        let mut live_start = 0_usize;
        for (seq, (order_key, _entry)) in hits.into_iter().enumerate() {
            let Some(segment) = Segment::decode(&order_key) else {
                return Ok(None);
            };
            if segment.seq != seq || segment.live_start != live_start {
                return Ok(None);
            }
            live_start += segment.live_len;
            let _ignored = blocks.insert(segment.block);
            segments.push(segment);
        }

        if blocks.len() != self.blocks.len()? {
            return Ok(None);
        }
        Ok(Some(segments))
    }

    /// The live characters of `start..end` served from the ordered index, or
    /// `None` when the index cannot be trusted and the caller must fall back.
    fn indexed_text(&self, start: usize, end: usize) -> Result<Option<String>, StoreError> {
        let Some(segments) = self.indexed_segments()? else {
            return Ok(None);
        };

        let mut out = String::new();
        let mut cached: Option<(BlockId, TextBlock)> = None;
        for segment in &segments {
            if segment.live_start.saturating_add(segment.live_len) <= start {
                continue;
            }
            if segment.live_start >= end {
                break;
            }

            if cached.as_ref().is_none_or(|(id, _)| *id != segment.block) {
                let Some(block) = self.blocks.get(&BlockKey::new(segment.block))? else {
                    return Ok(None);
                };
                cached = Some((segment.block, block.into_inner()));
            }
            let Some((_, block)) = cached.as_ref() else {
                return Ok(None);
            };

            // Re-validate the index entry against the block it names before a
            // single character is trusted: the block must be the one the key
            // claims, must be long enough for the window, and must contain
            // exactly the live count the key advertises.
            if block.start_id != segment.block || block.len() < segment.offset + segment.nodes {
                return Ok(None);
            }
            let text: String = block
                .text
                .chars()
                .skip(segment.offset)
                .take(segment.nodes)
                .enumerate()
                .filter(|(offset, _)| !tomb_get(&block.tombstones, segment.offset + offset))
                .map(|(_, content)| content)
                .collect();
            if text.chars().count() != segment.live_len {
                return Ok(None);
            }

            let from = start.saturating_sub(segment.live_start);
            let to = end.saturating_sub(segment.live_start).min(segment.live_len);
            out.push_str(slice_chars(&text, from, to));
        }
        Ok(Some(out))
    }

    /// The `(nodes, live)` totals the ordered index describes, or `None` when
    /// the index cannot be trusted. Test-only: the index/authority agreement
    /// assertion has no production caller.
    #[cfg(test)]
    fn index_cardinality(&self) -> Option<(usize, usize)> {
        let segments = self.indexed_segments().ok().flatten()?;
        Some(segments.iter().fold((0, 0), |(nodes, live), segment| {
            (nodes + segment.nodes, live + segment.live_len)
        }))
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
        self.rebuild_index()
    }
}

/// The fixed byte width of an ordered-index order key.
const ORDER_KEY_LEN: usize = 40;

/// One ordered-index entry: a maximal run of consecutive nodes of one stored
/// block that are also consecutive in document order.
///
/// A block usually contributes exactly one segment. It contributes more than
/// one only when a foreign node orders *inside* it — which the split rule
/// normally prevents, but which overlapping stored copies of one run (a split
/// that raced a remote unsplit copy) can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Segment {
    /// Live characters before this segment: the primary sort field, so an
    /// ascending scan of order keys is document order.
    live_start: usize,
    /// Position in document order. Ties `live_start` apart when a fully
    /// tombstoned segment contributes no characters.
    seq: usize,
    /// Live characters in this segment.
    live_len: usize,
    /// The owning block.
    block: BlockId,
    /// Where this segment starts inside the owning block's run.
    offset: usize,
    /// The segment's node count, tombstones included.
    nodes: usize,
}

impl Segment {
    /// The order key: fixed-width big-endian fields, most significant first, so
    /// the backend's byte order *is* document order.
    fn order_key(&self) -> [u8; ORDER_KEY_LEN] {
        let mut key = [0_u8; ORDER_KEY_LEN];
        key[0..8].copy_from_slice(&(self.live_start as u64).to_be_bytes());
        key[8..16].copy_from_slice(&(self.seq as u64).to_be_bytes());
        key[16..20].copy_from_slice(&(self.live_len as u32).to_be_bytes());
        key[20..24].copy_from_slice(&(self.offset as u32).to_be_bytes());
        key[24..28].copy_from_slice(&(self.nodes as u32).to_be_bytes());
        key[28..36].copy_from_slice(&self.block.replica.to_be_bytes());
        key[36..40].copy_from_slice(&self.block.counter.to_be_bytes());
        key
    }

    /// Parse an order key, rejecting anything self-inconsistent.
    fn decode(key: &[u8]) -> Option<Self> {
        let key: &[u8; ORDER_KEY_LEN] = key.try_into().ok()?;
        let field8 = |from: usize| -> usize {
            usize::try_from(u64::from_be_bytes(
                key[from..from + 8].try_into().unwrap_or([0; 8]),
            ))
            .unwrap_or(usize::MAX)
        };
        let field4 = |from: usize| -> u32 {
            u32::from_be_bytes(key[from..from + 4].try_into().unwrap_or([0; 4]))
        };
        let segment = Self {
            live_start: field8(0),
            seq: field8(8),
            live_len: field4(16) as usize,
            offset: field4(20) as usize,
            nodes: field4(24) as usize,
            block: BlockId::new(
                u64::from_be_bytes(key[28..36].try_into().unwrap_or([0; 8])),
                field4(36),
            ),
        };
        (segment.nodes > 0 && segment.live_len <= segment.nodes).then_some(segment)
    }
}

/// Split the document into ordered-index segments, and collect the tombstone
/// repairs that make the stored blocks agree with the authoritative join.
///
/// Ownership of a node is [`find_block`]'s rule — the tightest (most finely
/// split) covering block — so overlapping copies of one run never double-count
/// a node, and two replicas holding the same block set derive the same
/// segments.
///
/// Liveness comes from the TREE (delete-wins across every copy), not from the
/// owning block's bitmap; where the two disagree the bitmap is repaired, which
/// is what keeps an index-served read and an authoritative read identical.
#[expect(
    clippy::type_complexity,
    reason = "one call site; naming the repair list adds a type for no reader benefit"
)]
fn segment_document(
    loaded: &[LoadedBlock],
    tree: &FugueTree,
) -> Result<(Vec<Segment>, Vec<(usize, TextBlock)>), StoreError> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut repairs: BTreeMap<usize, TextBlock> = BTreeMap::new();
    let mut live_start = 0_usize;
    let mut open: Option<(usize, Segment)> = None;

    for raw in tree.ordered_ids() {
        let id = BlockId::from_raw(raw);
        let index = find_block(loaded, id).ok_or_else(|| invalid("ordered node has no block"))?;
        let lb = &loaded[index];
        let offset = (id.counter - lb.id.counter) as usize;
        let live = tree.node(raw).is_some_and(|node| node.value.is_some());

        if !live && !tomb_get(&lb.block.tombstones, offset) {
            let block = repairs.entry(index).or_insert_with(|| lb.block.clone());
            tomb_set(&mut block.tombstones, offset);
            tomb_trim(&mut block.tombstones);
        }

        match open.as_mut() {
            Some((open_index, segment))
                if *open_index == index && segment.offset + segment.nodes == offset =>
            {
                segment.nodes += 1;
                segment.live_len += usize::from(live);
            }
            _ => {
                if let Some((_, segment)) = open.take() {
                    live_start += segment.live_len;
                    segments.push(segment);
                }
                open = Some((
                    index,
                    Segment {
                        live_start,
                        seq: segments.len(),
                        live_len: usize::from(live),
                        block: lb.id,
                        offset,
                        nodes: 1,
                    },
                ));
            }
        }
    }
    if let Some((_, segment)) = open.take() {
        segments.push(segment);
    }

    Ok((segments, repairs.into_iter().collect()))
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

/// The ordered index and the positional read API.
///
/// These tests are deliberately written against the *observable* contract —
/// `text_range` / `char_at` agree with `get_text`, and a damaged index never
/// misroutes a positional WRITE — rather than against the key encoding, which
/// is an implementation detail free to change.
#[cfg(test)]
mod index_tests {
    use super::{blocks_containing, build_tree, BlockId, FugueText, UnorderedMap};
    use crate::collections::fugue::Rng;
    use crate::collections::Root;
    use crate::entities::Data;
    use crate::env;
    use crate::store::{MockedStorage, StorageAdaptor};

    fn doc_in<S: StorageAdaptor>(field_name: &str) -> FugueText<S> {
        FugueText {
            blocks: UnorderedMap::new_with_field_name_and_crdt_type(
                None,
                field_name,
                crate::collections::CrdtType::FugueText,
            ),
        }
    }

    /// Build a pseudo-random document: appends (which coalesce), mid-document
    /// inserts (which split) and deletes (which tombstone), so the resulting
    /// block set exercises every storage shape the index must describe.
    fn random_doc<S: StorageAdaptor>(doc: &mut FugueText<S>, rng: &mut Rng, ops: usize) {
        const WORDS: [&str; 5] = ["a", "bc", "déf", "ghij", "日本"];
        for _ in 0..ops {
            let len = doc.len().unwrap();
            if rng.below(4) == 0 && len > 0 {
                let start = rng.below(len);
                doc.delete_range(start, start + 1 + rng.below(3)).unwrap();
            } else {
                let pos = rng.below(len + 1);
                let replica = 1 + rng.below(3) as u64;
                doc.insert_str_with_replica(pos, replica, WORDS[rng.below(WORDS.len())])
                    .unwrap();
            }
        }
    }

    /// `text_range` is exactly the corresponding slice of `get_text`, over
    /// random documents and random ranges.
    #[test]
    fn text_range__equals_the_get_text_slice_over_random_documents() {
        env::reset_for_testing();
        type S = MockedStorage<880>;
        let mut rng = Rng::new(0x_1d_ec_0d_e5);

        for seed in 0..25_usize {
            let mut doc = doc_in::<S>(&format!("tr{seed}"));
            random_doc(&mut doc, &mut rng, 8);
            let text = doc.get_text().unwrap();
            let chars: Vec<char> = text.chars().collect();

            for _ in 0..12 {
                let a = rng.below(chars.len() + 3);
                let b = a + rng.below(chars.len() + 3);
                let expected: String = chars
                    .iter()
                    .skip(a.min(chars.len()))
                    .take(b.min(chars.len()).saturating_sub(a.min(chars.len())))
                    .collect();
                assert_eq!(
                    doc.text_range(a, b).unwrap(),
                    expected,
                    "seed {seed}: text_range({a}, {b}) over {text:?}"
                );
            }
        }
    }

    /// `char_at(i)` is `get_text().chars().nth(i)` for every `i`, including
    /// past the end.
    #[test]
    fn char_at__equals_chars_nth_including_out_of_bounds() {
        env::reset_for_testing();
        type S = MockedStorage<881>;
        let mut rng = Rng::new(0x_c4_a2_a7_11);

        for seed in 0..25_usize {
            let mut doc = doc_in::<S>(&format!("ca{seed}"));
            random_doc(&mut doc, &mut rng, 8);
            let text = doc.get_text().unwrap();

            for index in 0..text.chars().count() + 3 {
                assert_eq!(
                    doc.char_at(index).unwrap(),
                    text.chars().nth(index),
                    "seed {seed}: char_at({index}) over {text:?}"
                );
            }
        }
    }

    /// The full range is the whole document, and a range past the end clamps
    /// instead of erroring — a reader that errored when a concurrent remote
    /// delete shrank the document would be a guaranteed production bug.
    #[test]
    fn text_range__full_range_equals_get_text_and_clamps_past_the_end() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str(0, "hello").unwrap();
        doc.insert_str(2, "XY").unwrap();
        doc.delete(0).unwrap();

        let text = doc.get_text().unwrap();
        let len = doc.len().unwrap();
        assert_eq!(doc.text_range(0, len).unwrap(), text);
        assert_eq!(doc.text_range(0, len + 100).unwrap(), text);
        assert_eq!(doc.text_range(len, len + 100).unwrap(), "");
        assert_eq!(doc.text_range(len + 5, len + 100).unwrap(), "");
        assert_eq!(doc.text_range(3, 3).unwrap(), "");
        assert!(doc.text_range(3, 2).is_err(), "start > end must error");
    }

    /// Positions are `char` indices — Unicode scalar values, not bytes.
    #[test]
    fn text_range__counts_chars_not_bytes() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str(0, "héllo wörld").unwrap();
        assert_eq!(doc.text_range(0, 5).unwrap().chars().count(), 5);
        assert_eq!(doc.text_range(0, 5).unwrap(), "héllo");
        assert_eq!(doc.char_at(1).unwrap(), Some('é'));

        let mut cjk = Root::new(|| FugueText::new_with_field_name("cjk"));
        cjk.insert_str(0, "日本語のテキストです").unwrap();
        assert_eq!(cjk.text_range(0, 5).unwrap().chars().count(), 5);
        assert_eq!(cjk.text_range(0, 5).unwrap(), "日本語のテ");
        assert_eq!(cjk.char_at(2).unwrap(), Some('語'));
        assert_eq!(cjk.len().unwrap(), 10);
    }

    /// The index describes exactly the authoritative node set: every stored
    /// node is covered exactly once, and the covered live count is `len()`.
    #[test]
    fn index__cardinality_agrees_with_the_authoritative_node_set() {
        env::reset_for_testing();
        type S = MockedStorage<882>;
        let mut rng = Rng::new(0x_ca_2d_11_0a);

        for seed in 0..15_usize {
            let mut doc = doc_in::<S>(&format!("card{seed}"));
            random_doc(&mut doc, &mut rng, 8);

            let loaded = doc.load().unwrap();
            let tree = build_tree(&loaded).unwrap();
            let (nodes, live) = doc.index_cardinality().expect("index must be populated");
            assert_eq!(
                nodes,
                tree.nodes().count(),
                "seed {seed}: index must cover every authoritative node exactly once"
            );
            assert_eq!(live, doc.len().unwrap(), "seed {seed}: live count");

            // …and the reads really are SERVED from it, rather than silently
            // taking the authoritative fallback and passing for the wrong
            // reason.
            assert_eq!(
                doc.indexed_text(0, usize::MAX).unwrap(),
                Some(doc.get_text().unwrap()),
                "seed {seed}: reads must be served from the index"
            );
        }
    }

    /// The index is live under `MainStorage` too, not just the test mock —
    /// otherwise production would quietly keep paying the authoritative cost.
    #[test]
    fn index__is_populated_under_main_storage() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str(0, "hello").unwrap();
        doc.insert_str(2, "XY").unwrap();
        doc.delete(0).unwrap();

        let (_nodes, live) = doc.index_cardinality().expect("index must be populated");
        assert_eq!(live, doc.len().unwrap());
        assert_eq!(
            doc.indexed_text(0, usize::MAX).unwrap(),
            Some(doc.get_text().unwrap())
        );
    }

    /// CONSTRAINT 1. A dropped index entry must never make a positional WRITE
    /// target the wrong node: `parent`/`side` are SYNCED, so a wrong causal edge
    /// ships in the delta and diverges every replica permanently. The write path
    /// therefore resolves positions against the authoritative block set and
    /// never through the index.
    #[test]
    fn corrupt_index__never_misroutes_a_positional_write() {
        env::reset_for_testing();
        type S = MockedStorage<883>;

        for damage in 0..3_usize {
            let mut doc = doc_in::<S>(&format!("dmg{damage}"));
            doc.insert_str_with_replica(0, 1, "hello").unwrap();
            doc.insert_str_with_replica(2, 2, "XY").unwrap();
            doc.insert_str_with_replica(0, 3, "Z").unwrap();
            assert_eq!(doc.get_text().unwrap(), "ZheXYllo");

            let collection = doc.blocks.element().id();
            let keys: Vec<Vec<u8>> = S::index_range(
                collection,
                core::ops::Bound::Unbounded,
                core::ops::Bound::Unbounded,
                0,
                None,
            )
            .into_iter()
            .map(|(key, _)| key)
            .collect();
            assert!(!keys.is_empty(), "the index must be populated to damage it");

            match damage {
                // Drop one entry: positions computed from the index would shift.
                0 => {
                    let _ = S::index_remove(collection, &keys[0]);
                }
                // Corrupt an entry's payload without changing its cardinality.
                1 => {
                    let mut bogus = keys[keys.len() - 1].clone();
                    if let Some(last) = bogus.last_mut() {
                        *last ^= 0xff;
                    }
                    let _ = S::index_remove(collection, &keys[keys.len() - 1]);
                    let _ = S::index_put(collection, &bogus, collection);
                }
                // Wipe it entirely.
                _ => {
                    let _ = S::index_clear(collection);
                }
            }

            // A dropped or wiped entry is caught by the cardinality/density
            // validation, so the index is REFUSED rather than merely surviving
            // the damage by luck. (The mutated-payload case may still parse;
            // it is caught downstream, when the block it names fails to match.)
            if damage != 1 {
                assert!(
                    doc.indexed_segments().unwrap().is_none(),
                    "damage {damage}: a damaged index must be refused, not trusted"
                );
            }

            // Reads must refuse the damaged index and fall back, never serve a
            // wrong view from it.
            assert_eq!(
                doc.get_text().unwrap(),
                "ZheXYllo",
                "damage {damage}: a damaged index served a wrong read"
            );
            assert_eq!(doc.text_range(1, 4).unwrap(), "heX", "damage {damage}");
            assert_eq!(doc.char_at(3).unwrap(), Some('X'), "damage {damage}");
            assert_eq!(doc.len().unwrap(), 8, "damage {damage}");

            // The write must land on the character the AUTHORITATIVE order says
            // is at position 3 ('X'), whatever the index claims.
            doc.delete(3).unwrap();
            assert_eq!(
                doc.get_text().unwrap(),
                "ZheYllo",
                "damage {damage}: a damaged index misrouted a positional write"
            );

            // And the insert origin must be authoritative too.
            doc.insert_str_with_replica(1, 4, "Q").unwrap();
            assert_eq!(doc.get_text().unwrap(), "ZQheYllo", "damage {damage}");

            // Reads must be correct as well — either from a repaired index or
            // from the authoritative fallback, never from the damaged one.
            assert_eq!(doc.text_range(0, 4).unwrap(), "ZQhe");
            assert_eq!(doc.char_at(2).unwrap(), Some('h'));
        }
    }

    /// THE WRINKLE. A split racing a remote copy of the unsplit run leaves
    /// overlapping stored blocks — the stored form no longer satisfies
    /// "unbroken right-chain, foreign children only at endpoints". Ordered
    /// reads must still be exact, and the index must not double-count the
    /// nodes both copies define.
    #[test]
    fn overlapping_blocks__ordered_reads_stay_exact() {
        type A = MockedStorage<884>;
        type B = MockedStorage<885>;
        type M = MockedStorage<886>;
        env::reset_for_testing();

        let mut a = doc_in::<A>("ov");
        a.insert_str_with_replica(0, 2, "abcd").unwrap();
        let mut b = doc_in::<B>("ov");
        b.insert_str_with_replica(0, 2, "abcd").unwrap();

        // B coalesces onto the run; A splits it in two places.
        b.insert_str_with_replica(4, 2, "ef").unwrap();
        a.insert_str_with_replica(1, 1, "Q").unwrap();
        a.insert_str_with_replica(4, 1, "R").unwrap();

        let mut merged = doc_in::<M>("ov-m");
        merged.merge_blocks_from(&a).unwrap();
        merged.merge_blocks_from(&b).unwrap();

        // Overlap really is present in the STORED form.
        let loaded = merged.load().unwrap();
        let overlapping = loaded
            .iter()
            .filter(|lb| {
                blocks_containing(&loaded, BlockId::new(lb.id.replica, lb.id.counter)).len() > 1
            })
            .count();
        assert!(
            overlapping > 0,
            "this scenario is supposed to produce overlapping stored blocks"
        );

        let text = merged.get_text().unwrap();
        assert_eq!(text, "aQbcRdef");
        assert_eq!(merged.text_range(0, merged.len().unwrap()).unwrap(), text);
        for index in 0..text.chars().count() + 2 {
            assert_eq!(merged.char_at(index).unwrap(), text.chars().nth(index));
        }
        for start in 0..text.chars().count() {
            for end in start..text.chars().count() + 2 {
                let expected: String = text.chars().skip(start).take(end - start).collect();
                assert_eq!(merged.text_range(start, end).unwrap(), expected);
            }
        }

        let tree = build_tree(&loaded).unwrap();
        let (nodes, live) = merged.index_cardinality().expect("index populated");
        assert_eq!(nodes, tree.nodes().count(), "overlap must not double-count");
        assert_eq!(live, merged.len().unwrap());
    }

    /// `len` and `is_empty` agree with `get_text` on an indexed document.
    #[test]
    fn len__agrees_with_get_text_on_an_indexed_document() {
        env::reset_for_testing();
        type S = MockedStorage<887>;
        let mut rng = Rng::new(0x_5e_11_00_02);

        for seed in 0..15_usize {
            let mut doc = doc_in::<S>(&format!("len{seed}"));
            random_doc(&mut doc, &mut rng, 8);
            let text = doc.get_text().unwrap();
            assert_eq!(doc.len().unwrap(), text.chars().count());
            assert_eq!(doc.is_empty().unwrap(), text.is_empty());
        }
    }
}
