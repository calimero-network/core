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
//! ## No node-local derived state, on any path
//!
//! Every operation — read and write alike — derives its answer from the stored
//! blocks and the tree built from them. Nothing consults a cache, an index or a
//! validity marker, and that is a correctness requirement, not a simplification.
//!
//! **A write must not read derived state**, because `parent` and `side` are
//! *synced* fields: a position resolved through a stale index does not produce a
//! wrong local view that a rebuild repairs — it mints a wrong causal edge that
//! ships in the delta and diverges every replica permanently.
//!
//! **A read must not read node-local derived state either**, because the COST
//! would then depend on it. An ordered index over the [`StorageAdaptor`]
//! `index_*` seam is only ever warm on a node that has WRITTEN locally; a node
//! that acquired the document by sync has the same state and a cold index. A
//! read served from a warm index and a read served from the authoritative path
//! are the same logical operation at very different gas, so one replica can
//! complete a read-then-write guest call while another traps on `GasExhausted` —
//! divergence, not slowness. `crates/runtime/tests/metering_determinism.rs`
//! exists to catch exactly this class.
//!
//! An earlier revision of this file carried such an index, gated on a
//! `full_hash` validity marker, to make [`len`](FugueText::len) cost zero entity
//! loads and [`char_at`](FugueText::char_at) exactly one. It was removed rather
//! than repaired: the property above admits only two shapes — never read the
//! index, or rebuild it unconditionally before every read — and the second pays
//! the whole authoritative cost *plus* the index writes, so it is strictly worse
//! than not having one. Positional reads therefore cost `O(blocks + nodes)`,
//! like the whole-document read. Run-length blocks are what keep that
//! affordable: `blocks` counts *runs*, not characters.
//!
//! What survives from that work is [`tombstone_repairs`], which is not an index:
//! it writes the authoritative delete-wins join back into overlapping stored
//! copies of one run, so two replicas holding the same node set also hold the
//! same bytes for it.
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
    /// Test-only constructor: production code mints ids through
    /// [`next_counter`] and never assembles one by hand.
    #[cfg(test)]
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
        self.normalise_blocks()
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
        // NOTE (Constraint 1): no derived state is consulted here. `parent` and
        // `side` are synced fields, so a position resolved through anything but
        // the stored blocks could mint a wrong causal edge that ships in the
        // delta and diverges every replica permanently. `tree` above is built
        // from the authoritative block set.
        let count = end.min(tree.len()).saturating_sub(start);
        let mut targets: Vec<(u64, u32)> = Vec::with_capacity(count);
        for _ in 0..count {
            match tree.delete(start) {
                Ok(id) => targets.push(id),
                Err(_) => break,
            }
        }
        if targets.is_empty() {
            return self.normalise_blocks();
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
            self.put_block(BlockKey::new(loaded[index].id), block)?;
        }

        self.normalise_blocks()
    }

    /// The document text, tombstones excluded.
    ///
    /// Derived from the authoritative block set — see the module doc on why no
    /// read consults node-local derived state.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn get_text(&self) -> Result<String, StoreError> {
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
        let loaded = self.load()?;
        let text = build_tree(&loaded)?.values();
        Ok(slice_chars(&text, start, end).to_owned())
    }

    /// The character at `pos`, or `None` if `pos` is past the end.
    ///
    /// Positions are `char` indices — Unicode scalar values, not bytes and not
    /// grapheme clusters.
    ///
    /// Derived from the authoritative block set — see the module doc on why no
    /// read consults node-local derived state.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn char_at(&self, pos: usize) -> Result<Option<char>, StoreError> {
        let loaded = self.load()?;
        Ok(build_tree(&loaded)?.values().chars().nth(pos))
    }

    /// The number of visible characters.
    ///
    /// Derived from the authoritative block set — see the module doc on why no
    /// read consults node-local derived state.
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

    /// Write the node the tree just minted into the block store, extending an
    /// existing run where the new node continues it.
    ///
    /// # Why this does not split
    ///
    /// A mid-run insert lands as a LEFT child of a mid-run node, and the run's
    /// implicit intra-run edges stay valid, so nothing has to be split. This is
    /// provable rather than empirical, by comparing the two sides of the
    /// expansion in [`build_tree`]:
    ///
    /// * `build_tree` synthesises node `k` of run `(r, c, text)` as
    ///   `id = (r, c + k)`, `parent = (r, c + k - 1)`, `side = R` for `k > 0`.
    /// * Splitting that run before offset `f` used to store the tail as
    ///   `(r, c + f, text[f..])` with `parent = (r, c + f - 1)`, `side = R`, and
    ///   `build_tree` then synthesises its node `k - f` as `id = (r, c + k)`,
    ///   `parent = (r, c + k - 1)`, `side = R`.
    ///
    /// The two agree on id, parent, side and — the bitmap being rebased by the
    /// same offset — liveness, for every node. The split was therefore a pure
    /// storage-layout no-op: identical node set, identical tree, identical
    /// document order. It bought the invariant "a run's nodes are an unbroken
    /// right-chain with foreign children only at its endpoints", which the
    /// ordered index wanted; the index is gone (it made read gas depend on
    /// node-local cache warmth), so the invariant has no buyer and splitting was
    /// pure cost — roughly 2x RGA's row count on mid-document typing, because
    /// each insert wrote two or three entities instead of one and grew the block
    /// count linearly.
    ///
    /// Not splitting is also strictly better for merge: a split racing a remote
    /// copy of the unsplit run was the only way overlapping stored blocks arose
    /// from ordinary editing.
    fn materialize(&mut self, loaded: &[LoadedBlock], node: FugueNode) -> Result<(), StoreError> {
        let id = BlockId::from_raw(node.id);
        let parent = node.parent.map(BlockId::from_raw);

        // The new node continues an existing run of this replica: extend it
        // instead of minting a second entity. This is what makes sequential
        // typing cost one entity, not n. The appended node is live, and a
        // trailing zero bit is implicit, so the bitmap is untouched.
        //
        // Only a RIGHT child can continue a run, and only off the run's LAST
        // node — appending to a run re-parents the new node onto that last
        // node, which is the edge Fugue chose only in that case.
        if node.side == Side::R {
            if let Some(parent_id) = parent {
                if let Some(index) = find_block(loaded, parent_id) {
                    let lb = &loaded[index];
                    let offset = (parent_id.counter - lb.id.counter) as usize;
                    if offset + 1 == lb.len && coalesces_into(lb, id) {
                        let mut block = lb.block.clone();
                        block.text.push(node.value.unwrap_or('\u{fffd}'));
                        self.put_block(BlockKey::new(lb.id), block)?;
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
        self.put_block(BlockKey::new(id), block)?;
        Ok(())
    }

    /// Write one block, stamping its ENTRY element `CrdtType::FugueTextBlock`.
    ///
    /// Every write of a `TextBlock` goes through here, and the stamp is the
    /// whole point: an entry element is created untagged, and an untagged
    /// entity is reconciled by last-writer-wins in
    /// `Interface::try_merge_non_root`. `TextBlock` values are MUTABLE — a run
    /// grows in place when [`materialize`](Self::materialize) coalesces into it,
    /// and its tombstone bitmap is rewritten by ANY replica that deletes inside
    /// it — so two replicas routinely hold different values for one key, and LWW
    /// drops whichever side's edit loses. The tag routes that collision to
    /// [`join_block`] instead. (`ReplicatedGrowableArray` needs no
    /// such tag: an `RgaChar` is immutable once written, so its keys never
    /// collide on differing values.)
    fn put_block(&mut self, key: BlockKey, block: TextBlock) -> Result<(), StoreError> {
        use crate::entities::Data as _;

        let inherited = self.blocks.element().metadata.storage_type.clone();
        let _ignored = self.blocks.insert_with_storage_type_and_crdt_type(
            key,
            block,
            inherited,
            None,
            Some(CrdtType::FugueTextBlock),
        )?;
        Ok(())
    }

    /// Normalise the stored blocks against the authoritative join.
    ///
    /// Called **unconditionally** at the end of every mutating operation. Where
    /// two overlapping stored copies of one run disagree about a tombstone, the
    /// authoritative join (delete-wins, what [`build_tree`] computes) is written
    /// back into the owning copy, so two replicas that hold the same node set
    /// also hold the same BYTES for it. Cost is `O(nodes)` on a path that has
    /// already loaded every block and built the tree.
    fn normalise_blocks(&mut self) -> Result<(), StoreError> {
        let loaded = self.load()?;
        let tree = build_tree(&loaded)?;

        for (index, block) in tombstone_repairs(&loaded, &tree)? {
            self.put_block(BlockKey::new(loaded[index].id), block)?;
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
                join_block(&mut merged, incoming);
                if merged != mine {
                    self.put_block(key, merged)?;
                }
                continue;
            }

            // Absent from `self`'s live set: a tombstone means `self` deleted
            // this entity (delete wins, don't resurrect); otherwise it is new.
            if crate::index::Index::<S>::is_deleted(self.blocks.entry_id(&key))? {
                continue;
            }
            self.put_block(key, incoming)?;
        }
        self.normalise_blocks()
    }

    /// Join two stored ENTRIES of the block map, given as the raw bytes the
    /// storage layer holds for them.
    ///
    /// This is the leaf-merge entry point the SYNC path reaches, via
    /// `merge_by_crdt_type(CrdtType::FugueTextBlock, ..)` — see that variant's
    /// doc for why a leaf tag is needed at all. The join itself is
    /// [`join_block`], the same one [`merge_blocks_from`](Self::merge_blocks_from)
    /// applies, so the two paths cannot drift apart.
    ///
    /// The byte layout is an `UnorderedMap` entry's:
    /// `borsh(Entry<(BlockKey, TextBlock)>)`, i.e. the key and value followed by
    /// the entry's `Element` (whose only serialized field is its id). Both sides
    /// describe the same entity id, so the existing entry's `Element` is kept.
    ///
    /// # Errors
    /// Returns [`MergeError::SerializationError`] if either side is not a valid
    /// block-map entry.
    pub(crate) fn merge_block_entry_bytes(
        existing: &[u8],
        incoming: &[u8],
    ) -> Result<Vec<u8>, super::crdt_meta::MergeError> {
        use super::crdt_meta::MergeError;

        type BlockEntry = super::Entry<(BlockKey, TextBlock)>;

        let mut existing_entry: BlockEntry = borsh::from_slice(existing)
            .map_err(|error| MergeError::SerializationError(error.to_string()))?;
        let incoming_entry: BlockEntry = borsh::from_slice(incoming)
            .map_err(|error| MergeError::SerializationError(error.to_string()))?;

        join_block(&mut existing_entry.item.1, incoming_entry.item.1);

        borsh::to_vec(&existing_entry)
            .map_err(|error| MergeError::SerializationError(error.to_string()))
    }
}

/// The per-key join for two copies of one run: elementwise tombstone OR, longer
/// `text` wins.
///
/// A lattice join, so it is idempotent, commutative and associative — which is
/// what lets it run in any delivery order, on either side of a sync, without a
/// timestamp. The `text` rule is exact rather than a heuristic: a run only ever
/// grows at its tail, and only its own replica can grow it, so two copies of one
/// key are always prefixes of one another and the longer one strictly contains
/// the shorter. `parent` and `side` belong to the run's FIRST node, which never
/// changes once written, so they need no join at all.
fn join_block(mine: &mut TextBlock, incoming: TextBlock) {
    tomb_or(&mut mine.tombstones, &incoming.tombstones);
    if incoming.len() > mine.len() {
        mine.text = incoming.text;
    }
}

/// The tombstone repairs that make the stored blocks agree with the
/// authoritative join, as `(index into `loaded`, repaired block)`.
///
/// Ownership of a node is [`find_block`]'s rule — the tightest (most finely
/// split) covering block — so overlapping copies of one run never double-count a
/// node, and two replicas holding the same block set derive the same repairs.
///
/// Liveness comes from the TREE (delete-wins across every copy), not from the
/// owning block's bitmap; where the two disagree the bitmap is repaired, so two
/// replicas that agree on the node set also agree on the stored bytes.
fn tombstone_repairs(
    loaded: &[LoadedBlock],
    tree: &FugueTree,
) -> Result<Vec<(usize, TextBlock)>, StoreError> {
    let mut repairs: BTreeMap<usize, TextBlock> = BTreeMap::new();

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
    }

    Ok(repairs.into_iter().collect())
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

/// The next unused counter for `replica`, one past the highest node of that
/// replica the stored state mentions **anywhere**.
///
/// Derived from the stored set rather than from node-local state: runs are
/// tombstoned in place and never removed (Fugue nodes must survive deletion),
/// so the high-water mark survives every deletion and a counter is never
/// reused. Any future garbage collection of tombstoned blocks would break that
/// and must carry the high-water mark some other way.
///
/// # Why the union of all DEFINITIONS, not just the block spans
///
/// The block spans alone are a high-water mark only while no block is ever
/// shortened. A block IS shortened — `write_segments` splits a run in place —
/// and if a shortened copy ever wins over a longer one, the nodes past the new
/// end vanish from the span set and the replica re-mints ids it has already
/// used. Re-minting is worse than losing a node: `FugueTree::integrate` keeps
/// the FIRST definition of an id, so a peer still holding the long copy resolves
/// the id to the old character while the minter resolves it to the new one, and
/// the two never converge. That is divergence, not loss.
///
/// The leaf join (`CrdtType::FugueTextBlock`) is what makes the shortening
/// impossible — a block can only grow at merge — but the counter must not depend
/// on that being true. So the mark is taken over every *reference* to a node of
/// `replica` the stored state carries: the runs that define nodes, AND the
/// `parent` edges that point at them. A parent edge survives a split of the
/// block that defined its target (the split's own tail points back into the
/// head), so it keeps the mark up even for a node no surviving run spells out.
fn next_counter(replica: u64, loaded: &[LoadedBlock]) -> Result<u32, StoreError> {
    let mut next: u64 = 0;
    for lb in loaded {
        if lb.id.replica == replica {
            next = next.max(u64::from(lb.id.counter) + lb.len as u64);
        }
        if let Some(parent) = lb.block.parent {
            if parent.replica == replica {
                next = next.max(u64::from(parent.counter) + 1);
            }
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

    /// (3) A mid-run insert does NOT split the run: it adds exactly one block,
    /// the inserted run's own, and the document order is unchanged by that.
    ///
    /// Splitting here was a pure storage-layout no-op — see `materialize` for
    /// the proof — and it cost roughly 2x RGA's rows on mid-document typing.
    #[test]
    fn insert__mid_run_does_not_split_the_run() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str_with_replica(0, 7, "hello").unwrap();
        assert_eq!(stored(&doc).len(), 1);

        doc.insert_str_with_replica(2, 7, "X").unwrap();
        assert_eq!(doc.get_text().unwrap(), "heXllo");

        // Exactly ONE new entity: the run "hello" is untouched, and 'X' hangs
        // off its third node as a left child, which the implicit intra-run
        // edges already express.
        let blocks = stored(&doc);
        assert_eq!(
            blocks.len(),
            2,
            "a mid-run insert must add one block, not split into three"
        );

        let run = &blocks[0];
        assert_eq!(run.0, BlockId::new(7, 0));
        assert_eq!(run.1.text, "hello", "the run must survive intact");
        assert_eq!(run.1.parent, None);
        assert_eq!(run.1.side, BlockSide::R);

        let inserted = &blocks[1];
        assert_eq!(inserted.0, BlockId::new(7, 5));
        assert_eq!(inserted.1.text, "X");
        assert_eq!(inserted.1.parent, Some(BlockId::new(7, 2)));
        assert_eq!(inserted.1.side, BlockSide::L);
    }

    /// (3b) Repeated mid-document typing keeps the block count at one per
    /// distinct insertion point, and the text stays right.
    ///
    /// This is the shape the cost snapshot measures: under the old split rule
    /// each of these calls wrote two or three entities and the run count grew
    /// faster than the number of insertion points.
    #[test]
    fn insert__repeated_mid_document_typing_adds_one_block_each() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str_with_replica(0, 7, "abcdef").unwrap();
        assert_eq!(stored(&doc).len(), 1);

        // Type five characters mid-document with an ADVANCING caret, which is
        // what typing is. The first lands as a left child inside the run; each
        // later one is a right child of the previous, so they all COALESCE into
        // a single second run.
        for offset in 0..5 {
            doc.insert_str_with_replica(3 + offset, 7, "z").unwrap();
        }
        assert_eq!(doc.get_text().unwrap(), "abczzzzzdef");
        assert_eq!(
            stored(&doc).len(),
            2,
            "one untouched run plus one coalesced run of inserted text"
        );

        // A caret that does NOT advance re-inserts before the previous
        // character, so Fugue chains those left — one run each, and no split
        // rule could have merged them. Pinned so the distinction is explicit.
        let mut fixed = Root::new(FugueText::new);
        fixed.insert_str_with_replica(0, 8, "abcdef").unwrap();
        for _ in 0..3 {
            fixed.insert_str_with_replica(3, 8, "z").unwrap();
        }
        assert_eq!(fixed.get_text().unwrap(), "abczzzdef");
        assert_eq!(stored(&fixed).len(), 4);
    }

    /// (4) A mid-run insert produces byte-identical stored state on two
    /// independently built documents — no clock, no node-local input.
    #[test]
    fn insert__mid_run_stored_state_is_deterministic() {
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
            "a mid-run insert must be a pure function of its inputs"
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
    pub(super) struct Model {
        tree: FugueTree,
        next: BTreeMap<u64, u32>,
    }

    impl Model {
        pub(super) fn insert_str(&mut self, pos: usize, replica: u64, s: &str) {
            for (offset, content) in s.chars().enumerate() {
                let counter = self.next.entry(replica).or_default();
                let id = (replica, *counter);
                *counter += 1;
                let _ignored = self.tree.insert(pos + offset, content, id).unwrap();
            }
        }

        pub(super) fn delete_range(&mut self, start: usize, end: usize) {
            let count = end.min(self.tree.len()).saturating_sub(start);
            for _ in 0..count {
                if self.tree.delete(start).is_err() {
                    break;
                }
            }
        }

        pub(super) fn text(&self) -> String {
            self.tree.values()
        }

        fn nodes(&self) -> Vec<FugueNode> {
            self.tree.nodes().copied().collect()
        }

        /// The union of several models: every node, delete-wins per node, read
        /// through a fresh tree. `integrate` is order-independent and keeps a
        /// tombstone over a live copy, so this is exactly the CRDT join.
        pub(super) fn union(models: &[&Self]) -> String {
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
    pub(super) enum Op {
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
    pub(super) const ALPHABET: [Op; 5] = [
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

/// The positional read API.
///
/// These tests are deliberately written against the *observable* contract —
/// `text_range` / `char_at` agree with `get_text`, and the stored form covers
/// every authoritative node exactly once — rather than against any internal
/// layout, which is free to change.
#[cfg(test)]
mod positional_read_tests {
    use super::{
        blocks_containing, build_tree, find_block, BlockId, BlockKey, BlockSide, FugueText,
        TextBlock, UnorderedMap,
    };
    use crate::collections::fugue::Rng;
    use crate::collections::Root;
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

    /// Every stored node is covered by exactly one owning block, and the live
    /// nodes number `len()`.
    ///
    /// This is the invariant the ordered index used to assert about itself.
    /// It survives the index because it is a property of the STORED FORM, not
    /// of a cache: `find_block`'s tightest-cover rule must partition the node
    /// set even when overlapping copies of one run are present.
    #[test]
    fn stored_blocks__cover_every_authoritative_node_exactly_once() {
        env::reset_for_testing();
        type S = MockedStorage<882>;
        let mut rng = Rng::new(0x_ca_2d_11_0a);

        for seed in 0..15_usize {
            let mut doc = doc_in::<S>(&format!("card{seed}"));
            random_doc(&mut doc, &mut rng, 8);

            let loaded = doc.load().unwrap();
            let tree = build_tree(&loaded).unwrap();

            let mut owned: std::collections::BTreeSet<BlockId> = std::collections::BTreeSet::new();
            for raw in tree.ordered_ids() {
                let id = BlockId::from_raw(raw);
                let index = find_block(&loaded, id).expect("ordered node must have a block");
                let lb = &loaded[index];
                assert!(
                    lb.id.counter <= id.counter && id.counter < lb.id.counter + lb.len as u32,
                    "seed {seed}: owning block does not cover the node it owns"
                );
                assert!(owned.insert(id), "seed {seed}: node {id:?} covered twice");
            }
            assert_eq!(
                owned.len(),
                tree.nodes().count(),
                "seed {seed}: every authoritative node must be covered exactly once"
            );
            assert_eq!(
                doc.get_text().unwrap().chars().count(),
                doc.len().unwrap(),
                "seed {seed}: live count"
            );
        }
    }

    /// THE WRINKLE. Overlapping stored copies of one run — two blocks that both
    /// define the same nodes — must still read exactly, and the tightest-cover
    /// ownership rule must not double-count them.
    ///
    /// Local editing can no longer produce this: nothing splits a run any more,
    /// so a replica's blocks partition its own node space. It remains reachable
    /// from a PEER (a differently-blocked copy of the same nodes off the wire,
    /// including one written by an older or divergent implementation), which is
    /// exactly why `blocks_containing` and `find_block`'s tightest-cover rule
    /// are still load-bearing. So the overlap here is injected directly into the
    /// block map rather than staged through the insert path.
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

        // B coalesces onto the run; A edits inside it twice.
        b.insert_str_with_replica(4, 2, "ef").unwrap();
        a.insert_str_with_replica(1, 1, "Q").unwrap();
        a.insert_str_with_replica(4, 1, "R").unwrap();

        let mut merged = doc_in::<M>("ov-m");
        merged.merge_blocks_from(&a).unwrap();
        merged.merge_blocks_from(&b).unwrap();
        let text = merged.get_text().unwrap();
        assert_eq!(text, "aQbcRdef");

        // Inject a second, differently-blocked copy of the SAME nodes: the tail
        // of run (2,0) restated as its own block, with exactly the edges
        // `build_tree` synthesises for those nodes.
        merged
            .put_block(
                BlockKey::new(BlockId::new(2, 2)),
                TextBlock {
                    start_id: BlockId::new(2, 2),
                    text: "cdef".to_owned(),
                    parent: Some(BlockId::new(2, 1)),
                    side: BlockSide::R,
                    tombstones: Vec::new(),
                },
            )
            .unwrap();

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

        // ... and changes nothing about what the document says.
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
        let ordered = tree.ordered_ids();
        assert!(
            ordered
                .iter()
                .all(|raw| find_block(&loaded, BlockId::from_raw(*raw)).is_some()),
            "every ordered node must be owned by a stored block"
        );
        assert_eq!(
            ordered.len(),
            tree.nodes().count(),
            "overlap must not double-count"
        );

        // A delete must reach EVERY covering copy, or the join would resurrect
        // the node from the copy that still calls it live.
        merged.delete(3).unwrap();
        assert_eq!(merged.get_text().unwrap(), "aQbRdef");
    }

    /// `len` and `is_empty` agree with `get_text`.
    #[test]
    fn len__agrees_with_get_text() {
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

/// Convergence through the REAL receive path — [`Interface::apply_action`] —
/// rather than through [`FugueText::merge_blocks_from`].
///
/// # Why this module exists at all
///
/// Every other convergence test in this file calls `merge_blocks_from`
/// directly. That is the *join*, and it is correct — but it is not what a node
/// runs when a delta lands off the wire. A `TextBlock` lives as an entry of an
/// `UnorderedMap`, and an entry entity is created by `Element::new(id)`, which
/// stamps no `crdt_type`. Only the COLLECTION element carries
/// `CrdtType::FugueText`. So a value collision on the same `BlockKey` reaches
/// `Interface::try_merge_non_root` with `crdt_type: None` and is resolved by
/// last-writer-wins — the block join never runs.
///
/// RGA is immune because an `RgaChar` is immutable once written, so two
/// replicas can never hold different values for one key. `FugueText` mutates
/// existing keys — `materialize` grows a run in place when it coalesces, and
/// `delete_range` rewrites its tombstone bitmap from ANY replica — so a value
/// collision on one key is not exotic, it is the ordinary consequence of one
/// writer typing at the end of a run while another deletes inside it.
///
/// These tests drive that path.
#[cfg(test)]
mod apply_path_tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::model_tests::{Model, Op, ALPHABET};
    use super::FugueText;
    use crate::collections::fugue::Rng;
    use crate::collections::Root;
    use crate::delta::{clear_pending_delta, StorageDelta};
    use crate::env::{self, RuntimeEnv};
    use crate::interface::{ApplyContext, Interface};
    use crate::store::{Key, MainStorage};

    /// An in-memory main-storage backend owned by one replica. Same shape as
    /// `crate::testing`'s, kept local so these tests need no feature flag.
    type Store = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;

    /// Every replica in these tests is the same context — and it must be the
    /// NATIVE DEFAULT context (`[236; 32]`, see `env::mocked::context_id`).
    /// `collections::ROOT_ID` is a process-global `LazyLock<Id>` seeded from
    /// whatever `context_id()` returns the first time anything in the binary
    /// asks, so a test that installs a different context id poisons the root id
    /// for every other test in the process — every `Root::new` afterwards fails
    /// with `CannotCreateOrphan`.
    const CONTEXT_ID: [u8; 32] = [236_u8; 32];

    /// Both replicas must derive the SAME collection id or their actions build
    /// two parallel documents that never meet — see `INTERLEAVED_DOC_FIELD` in
    /// `tools/storage-cost`.
    const FIELD: &str = "apply_path_doc";

    fn new_store() -> Store {
        Rc::new(RefCell::new(HashMap::new()))
    }

    /// A [`RuntimeEnv`] routing all `MainStorage` I/O into `store`, under the
    /// given device id. The device is what `local_replica` derives a Fugue
    /// replica id from, so it is what makes two replicas mint distinct nodes.
    fn env_for(store: &Store, device: [u8; 32]) -> RuntimeEnv {
        let r = Rc::clone(store);
        let reader = Rc::new(move |key: &Key| r.borrow().get(&key.to_bytes()).cloned());
        let w = Rc::clone(store);
        let writer = Rc::new(move |key: Key, value: &[u8]| {
            w.borrow_mut()
                .insert(key.to_bytes(), value.to_vec())
                .is_some()
        });
        let rm = Rc::clone(store);
        let remover = Rc::new(move |key: &Key| rm.borrow_mut().remove(&key.to_bytes()).is_some());
        let mut account = device;
        account[1] = 0xAC;
        RuntimeEnv::new(reader, writer, remover, CONTEXT_ID, device, account)
    }

    /// Device id of replica `n`. Distinct in the first 8 bytes, which is the
    /// only part `local_replica` reads.
    fn device(n: u8) -> [u8; 32] {
        let mut id = [n; 32];
        id[..8].copy_from_slice(&u64::from(n).to_be_bytes());
        id
    }

    /// Run `f` in `store` under `device`, returning the delta its commit emits.
    fn edit(
        store: &Store,
        device: [u8; 32],
        f: impl FnOnce(&mut Root<FugueText<MainStorage>>),
    ) -> Vec<u8> {
        clear_pending_delta();
        env::with_runtime_env(env_for(store, device), || {
            let mut doc =
                Root::<FugueText<MainStorage>>::fetch().expect("document root should exist");
            f(&mut doc);
            doc.commit();
            env::take_last_artifact().expect("commit should emit a delta")
        })
    }

    /// Land `delta` in `store` the way the sync path does: decode it into
    /// actions and push each through `Interface::apply_action`, skipping the
    /// sender's root entry (which is not the receiver's to overwrite). Copied
    /// from `land_remote_char` in `tools/storage-cost/src/workloads.rs`.
    fn land(store: &Store, device: [u8; 32], delta: &[u8]) {
        let actions = match borsh::from_slice::<StorageDelta>(delta).expect("delta should decode") {
            StorageDelta::Actions(actions) => actions,
            StorageDelta::CausalActions { actions, .. } => actions,
        };
        env::with_runtime_env(env_for(store, device), || {
            for action in actions {
                if action.id().is_root() {
                    continue;
                }
                Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
                    .expect("remote apply_action should succeed");
            }
        });
    }

    /// The document text as `store` sees it.
    fn text_in(store: &Store, device: [u8; 32]) -> String {
        env::with_runtime_env(env_for(store, device), || {
            Root::<FugueText<MainStorage>>::fetch()
                .expect("document root should exist")
                .get_text()
                .expect("get_text should succeed")
        })
    }

    /// The scenario: one writer typing at the END of a run while another
    /// DELETES inside it — the collision that remains now that nothing splits.
    ///
    /// * A holds `(A,0) = "ab"`; A types `'c'` at the end, which COALESCES,
    ///   rewriting key `(A,0)` as `"abc"` with an empty bitmap, at HLC `t1`.
    /// * B deletes `'b'`, which rewrites the SAME key `(A,0)` — text `"ab"`,
    ///   tombstone bit 1 set — at `t2 > t1`.
    /// * Both replicas now hold a different value for key `(A,0)`.
    ///
    /// [`join_block`] ORs the bitmaps and takes the longer text, giving
    /// `"abc"` with bit 1 set, i.e. `"ac"`. The APPLY path without a leaf tag
    /// resolves it by LWW instead, and whichever side loses is erased: B's
    /// write wins and node `(A,2) = 'c'` is gone, or A's wins and the delete of
    /// `'b'` is gone. Both replicas still AGREE either way — this is not caught
    /// by a convergence check, only by checking the merged VALUE.
    ///
    /// (Until mid-run inserts stopped splitting, this scenario was staged as
    /// typing-at-the-end versus inserting-in-the-middle, because the split
    /// rewrote the same key. Nothing splits now, so the collision has to come
    /// from the tombstone bitmap, which ANY replica may rewrite.)
    #[test]
    fn coalesce_meeting_a_delete_on_the_apply_path_keeps_every_node() {
        // Deliberately NOT `env::reset_environment()`: these replicas own their
        // own `Store`s, and the reset clears PROCESS-global mocked state that
        // other tests in this binary are using concurrently.
        let (dev_a, dev_b) = (device(1), device(2));

        // Genesis, authored by A so its nodes are A's to coalesce into.
        let genesis = new_store();
        clear_pending_delta();
        env::with_runtime_env(env_for(&genesis, dev_a), || {
            let mut doc = Root::new(|| FugueText::<MainStorage>::new_with_field_name(FIELD));
            doc.insert_str(0, "ab").expect("seed insert should succeed");
            doc.commit();
            let _ignored = env::take_last_artifact();
        });

        // Both replicas start from the identical genesis bytes.
        let store_a = genesis;
        let store_b: Store = Rc::new(RefCell::new(store_a.borrow().clone()));

        // A appends 'c' -> coalesces into key (A,0), which becomes "abc".
        let delta_a = edit(&store_a, dev_a, |doc| {
            doc.insert(2, 'c').expect("append should succeed");
        });
        // B deletes 'b' -> rewrites key (A,0)'s tombstone bitmap.
        let delta_b = edit(&store_b, dev_b, |doc| {
            doc.delete(1).expect("delete should succeed");
        });

        land(&store_a, dev_a, &delta_b);
        land(&store_b, dev_b, &delta_a);

        let after_a = text_in(&store_a, dev_a);
        let after_b = text_in(&store_b, dev_b);
        assert_eq!(
            after_a, "ac",
            "replica A lost an edit: the remote delete's copy of block (A,0) \
             overwrote the coalesced run through the LWW branch of \
             try_merge_non_root instead of being joined"
        );
        assert_eq!(
            after_b, "ac",
            "replica B lost an edit: the remote coalesced copy of block (A,0) \
             overwrote the tombstone through the LWW branch of \
             try_merge_non_root instead of being joined"
        );
    }

    /// Run one two-replica scenario end to end through the APPLY path, and check
    /// both replicas against the model oracle.
    ///
    /// The differential counterpart of `model_tests::scenario`, which reconciles
    /// through `merge_blocks_from`. Identical script, identical oracle, different
    /// reconciliation path — which is the whole point: 825 green scenarios
    /// through the join said nothing about the path a node actually runs.
    fn scenario_via_apply(tag: &str, left: &[Op], right: &[Op]) {
        let (dev_a, dev_b) = (device(1), device(2));

        // Genesis: an identical, already-synchronised history on both replicas.
        // Seeded under replica 0 so neither writer's counter space starts used.
        let genesis = new_store();
        clear_pending_delta();
        env::with_runtime_env(env_for(&genesis, dev_a), || {
            let mut doc = Root::new(|| FugueText::<MainStorage>::new_with_field_name(FIELD));
            doc.insert_str_with_replica(0, 0, "seed")
                .expect("seed insert should succeed");
            doc.commit();
            let _ignored = env::take_last_artifact();
        });
        let (mut ma, mut mb) = (Model::default(), Model::default());
        ma.insert_str(0, 0, "seed");
        mb.insert_str(0, 0, "seed");

        let store_a = genesis;
        let store_b: Store = Rc::new(RefCell::new(store_a.borrow().clone()));

        let deltas_a: Vec<Vec<u8>> = left
            .iter()
            .map(|op| edit_op(&store_a, dev_a, 1, op, &mut ma))
            .collect();
        let deltas_b: Vec<Vec<u8>> = right
            .iter()
            .map(|op| edit_op(&store_b, dev_b, 2, op, &mut mb))
            .collect();

        for delta in &deltas_b {
            land(&store_a, dev_a, delta);
        }
        for delta in &deltas_a {
            land(&store_b, dev_b, delta);
        }

        let expected = Model::union(&[&ma, &mb]);
        assert_eq!(
            text_in(&store_a, dev_a),
            expected,
            "{tag}: replica A disagrees with the model after applying B's deltas \
             (left={left:?} right={right:?})"
        );
        assert_eq!(
            text_in(&store_b, dev_b),
            expected,
            "{tag}: replica B disagrees with the model after applying A's deltas \
             (left={left:?} right={right:?})"
        );
    }

    /// Apply one op to a replica AND to its oracle, returning the delta the
    /// commit emitted. Mirrors `model_tests::apply`, including the `pos % (len +
    /// 1)` clamping that keeps a random script in bounds.
    fn edit_op(
        store: &Store,
        device: [u8; 32],
        replica: u64,
        op: &Op,
        model: &mut Model,
    ) -> Vec<u8> {
        let len = model.text().chars().count();
        let delta = edit(store, device, |doc| match *op {
            Op::Ins(pos, text) => {
                doc.insert_str_with_replica(pos % (len + 1), replica, text)
                    .expect("insert should succeed");
            }
            Op::Del(start, span) => {
                let start = start % (len + 1);
                doc.delete_range(start, start + span)
                    .expect("delete should succeed");
            }
        });
        match *op {
            Op::Ins(pos, text) => model.insert_str(pos % (len + 1), replica, text),
            Op::Del(start, span) => {
                let start = start % (len + 1);
                model.delete_range(start, start + span);
            }
        }
        delta
    }

    /// EXHAUSTIVE over one op per replica — all 5 x 5 = 25 pairs of the model
    /// sweep's alphabet — reconciled through `apply_action`.
    ///
    /// One op per side is where the coalesce-vs-split collision lives (it needs
    /// exactly one local append and one remote mid-run insert), so this covers
    /// the failure class exhaustively rather than by sampling.
    #[test]
    fn exhaustive__all_single_op_pairs_through_the_apply_path() {
        let mut checked = 0_usize;
        for (a, left) in ALPHABET.iter().enumerate() {
            for (b, right) in ALPHABET.iter().enumerate() {
                scenario_via_apply(
                    &format!("ap1{a}{b}"),
                    core::slice::from_ref(left),
                    core::slice::from_ref(right),
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 25, "the single-op sweep must be exhaustive");
    }

    /// EXHAUSTIVE, not sampled: the same 5^4 = 625 two-op scripts
    /// `model_tests::exhaustive__all_two_op_interleavings_across_two_replicas`
    /// runs through `merge_blocks_from`, reconciled instead through
    /// `apply_action`.
    ///
    /// Running the full space here is affordable — the apply path turns out to
    /// be CHEAPER per scenario than the model sweep, because each replica's
    /// script is committed op by op into its own small in-memory store rather
    /// than replayed into three separate documents.
    #[test]
    fn exhaustive__all_two_op_interleavings_through_the_apply_path() {
        let mut checked = 0_usize;
        for index in 0..625_usize {
            let pick = |shift: u32| ALPHABET[(index / 5_usize.pow(shift)) % 5].clone();
            let left = [pick(0), pick(1)];
            let right = [pick(2), pick(3)];
            scenario_via_apply(&format!("ap2{index}"), &left, &right);
            checked += 1;
        }
        assert_eq!(checked, 625, "the sweep must be exhaustive, not sampled");
    }

    /// Differential fuzz against the model, reconciled through `apply_action`.
    ///
    /// Same 200 seeds, same generator and same seed base as
    /// `model_tests::fuzz__two_hundred_seeds_match_the_model`, so a failure here
    /// has a directly comparable `merge_blocks_from` twin.
    #[test]
    fn fuzz__two_hundred_seeds_match_the_model_through_the_apply_path() {
        const SEEDS: usize = 200;
        const WORDS: [&str; 4] = ["a", "bc", "d", "ef"];

        let mut rng = Rng::new(0x_f0_5e_ed_01);
        for seed in 0..SEEDS {
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
            scenario_via_apply(&format!("apfz{seed}"), &left, &right);
        }
    }
}
