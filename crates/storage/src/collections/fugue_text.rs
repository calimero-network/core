//! `FugueText` - a storage-backed, run-length-blocked Tree-Fugue text CRDT.
//!
//! Replaces [`ReplicatedGrowableArray`](super::ReplicatedGrowableArray), keeping
//! its wiring (an [`UnorderedMap`] of id-keyed entities, so CRDT merge,
//! tombstoning and re-key machinery are inherited) and ordering by Tree-Fugue,
//! which is proven non-interleaving on backward insertion where RGA interleaves.
//! The synced state is `(parent, side)` edges, as the pure
//! [`fugue`](super::fugue) module models them.
//!
//! One entity is one RUN, not one character: a [`TextBlock`] holds consecutive
//! nodes `(replica, counter), (replica, counter + 1), …`, the first hanging off
//! the stored `(parent, side)` and every later one the right child of its
//! predecessor. That implicit chain is why `insert_str` resolves the Fugue rule
//! once, for its first character, and writes each block it touches exactly once.
//! Runs stop growing at [`MAX_RUN_LEN`] nodes: uncapped, a whole document is one
//! block re-read, rewritten and shipped on every keystroke. The character that
//! overflows a full run opens a block already parented on that run's last node,
//! side right, so nothing is ever split.
//!
//! Ordering is *computed*: every operation, read and write alike, expands the
//! stored blocks into a [`FugueTree`](super::fugue::FugueTree) and asks it for
//! the order. Nothing consults a cache, an index or a validity marker, and that
//! is a correctness requirement. A write must not, because `parent` and `side`
//! are SYNCED fields and a position resolved through stale state mints a wrong
//! causal edge that ships in the delta and diverges every replica permanently. A
//! read must not either, because its gas would then depend on node-local cache
//! warmth, and one replica trapping on `GasExhausted` where another completes
//! the same call is divergence, not slowness. Positional reads therefore cost
//! `O(blocks + nodes)`, affordable only because `blocks` counts runs.
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

const MAX_RUN_LEN: usize = 256; // nodes per block; caps what one keystroke rewrites and ships
const COUNTER_EXHAUSTED: &str = "replica counter space exhausted"; // a replica ran out of u32 ids

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub(crate) enum BlockSide {
    /// Left child, ordered before the parent's own value.
    L,
    /// Right child, ordered after the parent's own value.
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
    /// Tombstones, one bit per NODE, LSB-first, trailing zero bytes trimmed so
    /// one tombstone set has exactly one encoding.
    ///
    /// Per node rather than per run because coalescing GROWS a run's `text`, so
    /// a whole-run flag could not say which nodes it covered; elementwise OR is
    /// then the join. A delete never drops the entity: Fugue nodes must survive
    /// deletion, since a tombstone can still parent live nodes.
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

/// Elementwise OR: the merge join for two copies of one run.
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
    /// Panics inside a state migration: node ids are minted from the node-local
    /// device id, so a migrate body would mint a different id per node and
    /// diverge the network. Seed with
    /// [`insert_str_with_replica`](Self::insert_str_with_replica) instead.
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
    /// The deterministic counterpart of [`insert_str`](Self::insert_str): the
    /// counter is derived from the stored block set and never from a clock, so
    /// fixing the replica makes the whole result a pure function of the inputs.
    /// That is what seeding a document inside a migration needs.
    ///
    /// The replica must be genuinely unique per writer; two writers sharing one
    /// mint colliding node ids and diverge. Production callers should prefer
    /// [`insert_str`](Self::insert_str), which derives it from the device.
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

        // Only the first character needs the insert rule: each later one is the
        // right child of the one before it, the edge a run already implies.
        let mut rest = s.chars();
        let Some(first) = rest.next() else {
            return Ok(());
        };

        let loaded = self.load()?;
        let mut tree = build_tree(&loaded)?;
        let counter = next_counter(replica, &loaded)?;
        let node = tree
            .insert(pos, first, (replica, counter))
            .map_err(|err| invalid(&err.to_string()))?;

        self.materialize(&loaded, node, rest.as_str())
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
    /// Deleting never splits: tombstones are per node, so a whole run is
    /// tombstoned in place rather than shattered into one entity per character.
    ///
    /// # Errors
    /// Returns an error if `start > end` or storage fails.
    pub fn delete_range(&mut self, start: usize, end: usize) -> Result<(), StoreError> {
        if start > end {
            return Err(invalid("start must be <= end"));
        }

        let loaded = self.load()?;
        let mut tree = build_tree(&loaded)?;

        // Deleting at `start` repeatedly walks forward: each tombstone removes
        // that character from the live sequence.
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

        // Exactly one stored block defines any node (see `find_block`), so one
        // bitmap write per node is the whole of the delete.
        let mut touched: BTreeMap<usize, TextBlock> = BTreeMap::new();
        for raw in targets {
            let id = BlockId::from_raw(raw);
            let index =
                find_block(&loaded, id).ok_or_else(|| invalid("deleted node has no block"))?;
            let block = touched
                .entry(index)
                .or_insert_with(|| loaded[index].block.clone());
            tomb_set(
                &mut block.tombstones,
                (id.counter - loaded[index].id.counter) as usize,
            );
        }

        for (index, mut block) in touched {
            tomb_trim(&mut block.tombstones);
            self.put_block(BlockKey::new(loaded[index].id), block)?;
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

    /// The characters in the half-open range `start..end`.
    ///
    /// Positions are `char` indices, not bytes and not grapheme clusters.
    /// Clamped in `end` like [`delete_range`](Self::delete_range): erroring
    /// because a concurrent remote delete shrank the document out from under
    /// the reader would be a guaranteed production bug.
    ///
    /// # Errors
    /// Returns an error if `start > end` or storage fails.
    pub fn text_range(&self, start: usize, end: usize) -> Result<String, StoreError> {
        if start > end {
            return Err(invalid("start must be <= end"));
        }
        let loaded = self.load()?;
        let text = build_tree(&loaded)?.values();
        Ok(text.chars().skip(start).take(end - start).collect())
    }

    /// The character at `pos`, or `None` if `pos` is past the end.
    ///
    /// Positions are `char` indices, not bytes and not grapheme clusters.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn char_at(&self, pos: usize) -> Result<Option<char>, StoreError> {
        let loaded = self.load()?;
        Ok(build_tree(&loaded)?.values().chars().nth(pos))
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

    /// The equivalence oracle for the batched path: one single-character
    /// [`insert_str_with_replica`](Self::insert_str_with_replica) per character.
    #[cfg(test)]
    fn insert_str_per_char(&mut self, pos: usize, replica: u64, s: &str) -> Result<(), StoreError> {
        for (offset, content) in s.chars().enumerate() {
            self.insert_str_with_replica(
                pos + offset,
                replica,
                content.encode_utf8(&mut [0_u8; 4]),
            )?;
        }
        Ok(())
    }

    /// Write the node the tree just minted, plus the `rest` of the string it
    /// heads: extend an existing run where the first node continues one, then
    /// chunk whatever is left into capped blocks, each written exactly once.
    ///
    /// Nothing is split. A mid-run insert lands as a LEFT child of a mid-run
    /// node, leaving the run's implicit intra-run edges valid; a split tail
    /// would expand through [`build_tree`] to the identical id, parent, side and
    /// liveness for every node, so splitting only ever cost extra entities and
    /// was the one way ordinary editing produced overlapping stored blocks.
    fn materialize(
        &mut self,
        loaded: &[LoadedBlock],
        node: FugueNode,
        rest: &str,
    ) -> Result<(), StoreError> {
        let id = BlockId::from_raw(node.id);

        // Appended nodes are live and a trailing zero bit is implicit, so
        // extending a run leaves its bitmap untouched.
        let (mut key, mut block, filled) = match coalesce_target(loaded, node, id) {
            Some(lb) => (BlockKey::new(lb.id), lb.block.clone(), lb.len),
            None => (
                BlockKey::new(id),
                TextBlock {
                    start_id: id,
                    text: String::new(),
                    parent: node.parent.map(BlockId::from_raw),
                    side: node.side.into(),
                    tombstones: Vec::new(),
                },
                0,
            ),
        };
        block.text.push(node.value.unwrap_or('\u{fffd}'));

        let mut chars = rest.chars();
        let mut room = MAX_RUN_LEN - filled - 1;
        let mut last = id.counter;
        loop {
            for content in chars.by_ref().take(room) {
                block.text.push(content);
                last = bump(last)?;
            }
            self.put_block(key, block)?;

            let Some(content) = chars.next() else {
                return Ok(());
            };
            // Past the cap: the new block is parented on the full run's last
            // node, side right, the edge the intra-run chain would have carried.
            let start = BlockId {
                replica: id.replica,
                counter: bump(last)?,
            };
            block = TextBlock {
                start_id: start,
                text: content.to_string(),
                parent: Some(BlockId {
                    replica: id.replica,
                    counter: last,
                }),
                side: BlockSide::R,
                tombstones: Vec::new(),
            };
            key = BlockKey::new(start);
            last = start.counter;
            room = MAX_RUN_LEN - 1;
        }
    }

    /// Write one block, stamping its ENTRY element `CrdtType::FugueTextBlock`.
    ///
    /// Every write of a `TextBlock` goes through here, and the stamp is the
    /// whole point: an entry element is created untagged, and an untagged entity
    /// is reconciled by last-writer-wins in `Interface::try_merge_non_root`.
    /// `TextBlock` values are MUTABLE under one key (a run grows in place when
    /// [`materialize`](Self::materialize) coalesces into it, and any replica
    /// deleting inside it rewrites its bitmap), so LWW would drop whichever
    /// side's edit loses. The tag routes that collision to [`join_block`].
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
    /// The per-key join is [`join_block`], a genuine lattice join rather than an
    /// LWW heuristic; a block `self` removed at the storage layer stays removed.
    /// Generic over `S2` so a cross-store merge is testable.
    pub(crate) fn merge_blocks_from<S2: StorageAdaptor>(
        &mut self,
        other: &FugueText<S2>,
    ) -> Result<(), StoreError> {
        for (key, incoming) in other.blocks.entries()? {
            // Propagate a read error rather than treating it as "absent",
            // which would re-insert over live data.
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
        Ok(())
    }

    /// Join two stored ENTRIES of the block map, as the raw bytes the storage
    /// layer holds for them.
    ///
    /// The leaf-merge entry point the SYNC path reaches, via
    /// `merge_by_crdt_type(CrdtType::FugueTextBlock, ..)`. The join is
    /// [`join_block`], the same one
    /// [`merge_blocks_from`](Self::merge_blocks_from) applies, so the two paths
    /// cannot drift apart. Both sides describe the same entity id, so the
    /// existing entry's `Element` is kept.
    ///
    /// # Errors
    /// Returns [`MergeError::SerializationError`] if either side is not a valid
    /// block-map entry.
    pub(crate) fn merge_block_entry_bytes(
        existing: &[u8],
        incoming: &[u8],
    ) -> Result<Vec<u8>, super::crdt_meta::MergeError> {
        use super::crdt_meta::MergeError;

        // `(value, key)`, the order `UnorderedMap::insert_with_storage_type`
        // stores an entry in. The reverse order decodes SUCCESSFULLY, because
        // a `TextBlock` opens with a 12-byte `start_id` and a `BlockKey` is 12
        // bytes, so getting it wrong is a silent bad join, not a decode error.
        type BlockEntry = super::Entry<(TextBlock, BlockKey)>;

        let mut existing_entry: BlockEntry = borsh::from_slice(existing)
            .map_err(|error| MergeError::SerializationError(error.to_string()))?;
        let incoming_entry: BlockEntry = borsh::from_slice(incoming)
            .map_err(|error| MergeError::SerializationError(error.to_string()))?;

        join_block(&mut existing_entry.item.0, incoming_entry.item.0);

        borsh::to_vec(&existing_entry)
            .map_err(|error| MergeError::SerializationError(error.to_string()))
    }
}

/// The per-key join for two copies of one run: elementwise tombstone OR, and
/// the rest of the record is the maximum under `(node count, text, parent, side)`.
///
/// Honest copies are prefixes of one another with equal `parent` and `side`, so
/// the maximum is the longer run. Any other pair comes from a peer that reused a
/// counter or sent bad bytes; taking the record as a unit keeps that convergent.
fn join_block(mine: &mut TextBlock, incoming: TextBlock) {
    tomb_or(&mut mine.tombstones, &incoming.tombstones);
    fn rank(b: &TextBlock) -> (usize, &str, Option<BlockId>, BlockSide) {
        (b.len(), &b.text, b.parent, b.side)
    }
    if rank(&incoming) > rank(mine) {
        mine.text = incoming.text;
        mine.parent = incoming.parent;
        mine.side = incoming.side;
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
/// Requires the node to continue the run's own id sequence on the same replica;
/// `next_counter` mints strictly past every node the replica has defined, so an
/// absorbed id can never collide with an existing one. Absorption stops at
/// [`MAX_RUN_LEN`], after which a full block's text is frozen and only its
/// tombstone bits can still change.
fn coalesces_into(lb: &LoadedBlock, id: BlockId) -> bool {
    lb.len < MAX_RUN_LEN
        && lb.id.replica == id.replica
        && u64::from(lb.id.counter) + lb.len as u64 == u64::from(id.counter)
}

/// The stored run the freshly minted `node` continues, if it continues one.
///
/// Only a RIGHT child can, and only off the run's LAST node - appending to a
/// run re-parents the new node onto that last node, which is the edge Fugue
/// chose only in that case.
fn coalesce_target(loaded: &[LoadedBlock], node: FugueNode, id: BlockId) -> Option<&LoadedBlock> {
    if node.side != Side::R {
        return None;
    }
    let parent = BlockId::from_raw(node.parent?);
    let lb = loaded.get(find_block(loaded, parent)?)?;
    let offset = (parent.counter - lb.id.counter) as usize;
    (offset + 1 == lb.len && coalesces_into(lb, id)).then_some(lb)
}

/// One past `counter`, erroring where [`next_counter`] would have.
fn bump(counter: u32) -> Result<u32, StoreError> {
    counter
        .checked_add(1)
        .ok_or_else(|| invalid(COUNTER_EXHAUSTED))
}

/// The block whose run covers `id`, if the stored state defines that node.
///
/// There is at most one: a replica's runs PARTITION its counter space. A new
/// block starts at [`next_counter`], past every span the replica has defined;
/// coalescing extends a run only at that same high-water mark, so it grows only
/// into unclaimed space; and neither [`join_block`] (which takes the longest
/// `text`, and every copy is a prefix of the frozen final one) nor
/// `delete_range` (bitmaps only) can change a run's length afterwards. So the
/// last block starting at or before `id` is the only candidate.
fn find_block(loaded: &[LoadedBlock], id: BlockId) -> Option<usize> {
    let position = loaded.partition_point(|lb| lb.id <= id).checked_sub(1)?;
    let lb = loaded.get(position)?;
    (lb.id.replica == id.replica
        && u64::from(id.counter) < u64::from(lb.id.counter) + lb.len as u64)
        .then_some(position)
}

/// The next unused counter for `replica`, one past the highest node of that
/// replica the stored state mentions ANYWHERE.
///
/// Derived from the stored set rather than from node-local state: runs are
/// tombstoned in place and never removed, so the high-water mark survives every
/// deletion and a counter is never reused. Garbage collecting tombstoned blocks
/// would break that and would have to carry the mark some other way.
///
/// The mark covers every reference, `parent` edges included, not just the runs
/// that define nodes of `replica`. Re-minting a used id is worse than losing a
/// node: `FugueTree::integrate` keeps the FIRST definition, so a peer holding
/// the older copy resolves that id to a different character than the minter and
/// the two never converge. A `parent` edge outlives the run defining its target,
/// so it holds the mark up even for a node no surviving run spells out.
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
    u32::try_from(next).map_err(|_| invalid(COUNTER_EXHAUSTED))
}

/// Expand the stored blocks into a Fugue tree.
///
/// Node `i` of a run is `(replica, counter + i)`; the first hangs off the
/// stored `(parent, side)`, every later one is the right child of its
/// predecessor. Tombstoned nodes are still emitted: they take part in the
/// traversal and can still parent live nodes.
///
/// Every block is expanded at its FULL length and duplicate node ids are folded
/// together by [`FugueTree::integrate`], which is order-independent and keeps
/// the tombstone when a node is defined both live and dead. Overlapping
/// definitions of one node agree on value, parent and side by construction, so
/// this join is exact.
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
    use super::{join_block, tomb_set, BlockId, BlockSide, FugueText, TextBlock, MAX_RUN_LEN};
    use crate::collections::fugue::Rng;
    use crate::collections::{FugueTextSimple, Root, UnorderedMap};
    use crate::env;
    use crate::store::{MockedStorage, StorageAdaptor};

    const JOIN_LAW_MAX_NODES: usize = 12; // longest generated run: past one bitmap byte
    const JOIN_LAW_POOL: [char; 5] = ['a', 'b', 'z', 'é', '日']; // mixed widths: bytes != nodes
    const JOIN_LAW_SEED: u64 = 0x_5f_09_3e_10;
    const JOIN_LAW_ROUNDS: usize = 500;
    const CAP_POOL: [char; 4] = ['a', 'é', '日', 'z']; // mixed widths: bytes != nodes
    const DIFFERENTIAL_SEED: u64 = 0x_d1_ff_5e_ed;
    const DIFFERENTIAL_ROUNDS: usize = 200;
    const DIFFERENTIAL_MAX_NODES: usize = 800; // the control is quadratic in this; then deletes only

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

    /// Stored blocks sorted by id: the canonical form for equality assertions.
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

    /// Round trip, and the run is ONE entity, not five.
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

    /// Append coalescing: the whole point of blocks.
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

    /// A mid-run insert does NOT split the run: it adds exactly one block, the
    /// inserted run's own, and document order is unchanged by that.
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

    /// Repeated mid-document typing keeps the block count at one per distinct
    /// insertion point, and the text stays right.
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
        // character, so Fugue chains those left: one run each.
        let mut fixed = Root::new(FugueText::new);
        fixed.insert_str_with_replica(0, 8, "abcdef").unwrap();
        for _ in 0..3 {
            fixed.insert_str_with_replica(3, 8, "z").unwrap();
        }
        assert_eq!(fixed.get_text().unwrap(), "abczzzdef");
        assert_eq!(stored(&fixed).len(), 4);
    }

    /// A mid-run insert produces byte-identical stored state on two
    /// independently built documents: no clock, no node-local input.
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

    /// A delete inside a run tombstones the node in place; text and len agree,
    /// and the run is NOT shattered.
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

    /// Deleting a whole run leaves ONE tombstoned entity, not `n`.
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

    /// A run that GREW by coalescing, merged with a shorter copy the peer
    /// tombstoned, must tombstone only the nodes the deleter actually saw; a
    /// per-run flag would silently eat the appended node.
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

    /// A run that one replica SPLIT and the other COALESCED must, when merged,
    /// leave no node uncovered. Truncating a run where the next run of the same
    /// replica begins assumes the two copies partition exactly, which coalescing
    /// breaks.
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

    /// Backward insertion does not interleave, driven through the storage API.
    /// This is the property RGA lacks.
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

    /// Three concurrent replicas merged in all six orders read the same.
    ///
    /// The order permuted is the MERGE order: permuting `blocks.insert` calls
    /// would prove nothing, since `load` sorts by id and discards it.
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

    /// Positions are char indices, not byte offsets.
    #[test]
    fn positions_are_char_indices_not_byte_offsets() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str(0, "héllo wörld").unwrap();
        doc.insert(5, '!').unwrap();
        assert_eq!(doc.get_text().unwrap(), "héllo! wörld");
    }

    /// The merge-mode guard fires for `insert`.
    #[test]
    #[should_panic(expected = "migration")]
    fn insert_panics_during_migration() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        env::with_merge_mode(|| {
            let _ = doc.insert(0, 'H');
        });
    }

    /// The merge-mode guard fires for `insert_str`.
    #[test]
    #[should_panic(expected = "migration")]
    fn insert_str_panics_during_migration() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        env::with_merge_mode(|| {
            let _ = doc.insert_str(0, "Hi");
        });
    }

    /// The deterministic replay API stays usable inside a migration.
    #[test]
    fn insert_str_with_replica_is_allowed_during_migration() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        env::with_merge_mode(|| {
            doc.insert_str_with_replica(0, 7, "H").unwrap();
        });
        assert_eq!(doc.len().unwrap(), 1);
    }

    /// Every LOCAL write is a lattice-superset of the block it overwrites, the
    /// premise `Interface::apply_non_root_entry` rests on when it restricts the
    /// [`CrdtType::FugueTextBlock`] join arm to `WriteOrigin::Applied`: a run
    /// only ever grows and tombstone bits only accumulate, so skipping the join
    /// locally cannot drop a node. Break either half and this goes red before
    /// that restriction starts losing data.
    #[test]
    fn local_writes__are_lattice_supersets_of_what_they_overwrite() {
        /// Assert every block that survived the write absorbed its predecessor.
        fn assert_superset(
            before: &[(BlockId, TextBlock)],
            after: &[(BlockId, TextBlock)],
            what: &str,
        ) {
            for (id, old) in before {
                let (_, new) = after.iter().find(|(key, _)| key == id).unwrap_or_else(|| {
                    panic!("{what}: block {id:?} vanished; a local write must never drop one")
                });
                let mut joined = old.clone();
                join_block(&mut joined, new.clone());
                assert_eq!(
                    &joined, new,
                    "{what}: block {id:?} is not a lattice-superset of the copy it \
                     overwrote, so skipping the join on WriteOrigin::Local would lose data"
                );
            }
        }

        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str(0, "hello").unwrap();

        // Coalescing: grows a run's `text` in place.
        let before = stored(&doc);
        doc.insert(5, '!').unwrap();
        assert_superset(&before, &stored(&doc), "append coalescing");

        // Mid-run insert: mints a new block and leaves the covering run alone.
        let before = stored(&doc);
        doc.insert(2, 'X').unwrap();
        assert_superset(&before, &stored(&doc), "mid-run insert");

        // Delete: sets tombstone bits, never shortens `text`.
        let before = stored(&doc);
        doc.delete(1).unwrap();
        assert_superset(&before, &stored(&doc), "delete");

        let before = stored(&doc);
        doc.delete_range(0, 3).unwrap();
        assert_superset(&before, &stored(&doc), "delete_range");
    }

    /// A pseudo-random copy of the run starting at `start_id`.
    ///
    /// Every field but the key varies, since a peer's bytes are not trusted; the
    /// bitmap may set bits past this copy's own text, as a short copy of a run
    /// that grew elsewhere does.
    fn random_block(rng: &mut Rng, start_id: BlockId) -> TextBlock {
        let text: String = (0..rng.below(JOIN_LAW_MAX_NODES + 1))
            .map(|_| JOIN_LAW_POOL[rng.below(JOIN_LAW_POOL.len())])
            .collect();
        let mut tombstones = Vec::new();
        for offset in 0..JOIN_LAW_MAX_NODES {
            if rng.below(4) == 0 {
                tomb_set(&mut tombstones, offset);
            }
        }
        TextBlock {
            start_id,
            text,
            parent: (rng.below(2) == 0).then_some(start_id),
            side: if rng.below(2) == 0 {
                BlockSide::L
            } else {
                BlockSide::R
            },
            tombstones,
        }
    }

    fn joined(mine: &TextBlock, incoming: &TextBlock) -> TextBlock {
        let mut out = mine.clone();
        join_block(&mut out, incoming.clone());
        out
    }

    /// The join laws at one point of the block space, asserted on the borsh
    /// bytes because the Merkle hash is taken over exactly those - two replicas
    /// that agree on the value but not on its encoding still diverge.
    fn check_join_laws(what: &str, a: &TextBlock, b: &TextBlock, c: &TextBlock) {
        let bytes = |block: &TextBlock| borsh::to_vec(block).unwrap();

        assert_eq!(
            bytes(&joined(a, b)),
            bytes(&joined(b, a)),
            "{what}: join is not commutative for {a:?} and {b:?}"
        );
        assert_eq!(
            bytes(&joined(&joined(a, b), c)),
            bytes(&joined(a, &joined(b, c))),
            "{what}: join is not associative for {a:?}, {b:?} and {c:?}"
        );
        for block in [a, b, c] {
            assert_eq!(
                bytes(&joined(block, block)),
                bytes(block),
                "{what}: join is not idempotent for {block:?}"
            );
            assert_ne!(
                joined(a, block).tombstones.last(),
                Some(&0),
                "{what}: the joined bitmap kept a trailing zero byte, so two equal \
                 states would serialise to different bytes"
            );
        }
    }

    /// The join is a lattice join for ARBITRARY copies of one key, not only the
    /// prefix pairs ordinary editing produces. Equal-length-different-text and
    /// longer-non-prefix are pinned explicitly, because a rule keyed on node
    /// count alone resolves neither.
    #[test]
    fn join_block__is_commutative_associative_and_idempotent() {
        let id = BlockId::new(7, 3);
        let block = |text: &str, tombstones: Vec<u8>| TextBlock {
            start_id: id,
            text: text.to_owned(),
            parent: None,
            side: BlockSide::R,
            tombstones,
        };
        check_join_laws(
            "equal-length and non-prefix fixtures",
            &block("ab", vec![0b0000_0001]),
            &block("cd", vec![0b0000_0010]),
            &block("cdef", Vec::new()),
        );

        let mut seeds = Rng::new(JOIN_LAW_SEED);
        for _ in 0..JOIN_LAW_ROUNDS {
            let seed = seeds.next_u64();
            let mut rng = Rng::new(seed);
            check_join_laws(
                &format!("seed {seed:#x}"),
                &random_block(&mut rng, id),
                &random_block(&mut rng, id),
                &random_block(&mut rng, id),
            );
        }
    }

    /// `count` characters drawn cyclically from [`CAP_POOL`].
    fn cap_text(count: usize) -> String {
        (0..count).map(|i| CAP_POOL[i % CAP_POOL.len()]).collect()
    }

    /// Typing past the cap opens a SECOND block instead of growing the first
    /// for ever, and the new block hangs off the last node of the full one.
    #[test]
    fn insert__stops_coalescing_at_the_run_cap() {
        env::reset_for_testing();
        let mut doc = doc_in::<MockedStorage<875>>("cap-typing");
        let expected = cap_text(MAX_RUN_LEN + 1);
        for (i, content) in expected.chars().enumerate() {
            doc.insert_str_with_replica(i, 7, content.encode_utf8(&mut [0_u8; 4]))
                .unwrap();
        }

        assert_eq!(doc.get_text().unwrap(), expected);
        let blocks = stored(&doc);
        assert_eq!(blocks.len(), 2, "the run must stop coalescing at the cap");
        assert_eq!(blocks[0].1.text.chars().count(), MAX_RUN_LEN);
        assert_eq!(blocks[1].1.text.chars().count(), 1);
        assert_eq!(
            blocks[1].1.parent,
            Some(BlockId::new(7, u32::try_from(MAX_RUN_LEN - 1).unwrap())),
            "the second block hangs off the last node of the full one"
        );
        assert_eq!(blocks[1].1.side, BlockSide::R);
    }

    /// A paste longer than the cap becomes several capped blocks whose ids stay
    /// contiguous, and one more append still lands in the last of them - which
    /// is what proves `next_counter` survived the chunking.
    #[test]
    fn insert_str__chunks_a_long_paste_into_capped_blocks() {
        env::reset_for_testing();
        type S = MockedStorage<876>;
        let mut doc = doc_in::<S>("cap-paste");
        let paste = cap_text(3 * MAX_RUN_LEN + 5);
        doc.insert_str_with_replica(0, 7, &paste).unwrap();

        assert_eq!(doc.get_text().unwrap(), paste);
        let blocks = stored(&doc);
        assert_eq!(blocks.len(), 4, "the paste must be chunked at the cap");

        let mut counter = 0_u32;
        for (id, block) in &blocks {
            let len = block.text.chars().count();
            assert!(len <= MAX_RUN_LEN, "block {id:?} holds {len} nodes");
            assert_eq!(id.counter, counter, "chunk ids must stay contiguous");
            counter += u32::try_from(len).unwrap();
        }
        assert_eq!(usize::try_from(counter).unwrap(), paste.chars().count());

        doc.insert_str_with_replica(paste.chars().count(), 7, "!")
            .unwrap();
        let after = stored(&doc);
        assert_eq!(after.len(), 4, "the append must extend the last chunk");
        assert_eq!(after[3].1.text.chars().count(), 5 + 1);
    }

    /// Once a block is full its `text` never changes again: later appends,
    /// mid-run inserts and deletes may only move tombstone bits.
    #[test]
    fn full_block_text_is_frozen_under_later_edits() {
        env::reset_for_testing();
        type S = MockedStorage<877>;
        let mut doc = doc_in::<S>("cap-frozen");
        let run = cap_text(MAX_RUN_LEN);
        for (i, content) in run.chars().enumerate() {
            doc.insert_str_with_replica(i, 7, content.encode_utf8(&mut [0_u8; 4]))
                .unwrap();
        }
        let first = stored(&doc)[0].clone();
        assert_eq!(first.1.text.chars().count(), MAX_RUN_LEN);

        let frozen = |doc: &FugueText<S>, what: &str| {
            let now = stored(doc);
            assert_eq!(now[0].0, first.0);
            assert_eq!(
                now[0].1.text, first.1.text,
                "{what} rewrote the bytes of a full block"
            );
        };

        doc.insert_str_with_replica(MAX_RUN_LEN, 7, "xy").unwrap();
        frozen(&doc, "an append past the cap");
        doc.insert_str_with_replica(3, 7, "Q").unwrap();
        frozen(&doc, "a mid-run insert");
        doc.delete_range(1, 4).unwrap();
        frozen(&doc, "a delete");
        assert_ne!(
            stored(&doc)[0].1.tombstones,
            first.1.tombstones,
            "the delete must still have landed, as tombstone bits"
        );
    }

    /// The same random script driven into `FugueText` and into the blockless
    /// [`FugueTextSimple`] must read back the same text after every step. The
    /// control has no runs, so it is an oracle for the whole block layer.
    #[test]
    fn random_scripts_agree_with_the_blockless_control() {
        env::reset_for_testing();
        let mut blocked = doc_in::<MockedStorage<878>>("differential");
        let mut simple = FugueTextSimple::<MockedStorage<879>>::new_with_field_name_internal(
            None,
            "differential",
        );

        let mut rng = Rng::new(DIFFERENTIAL_SEED);
        let mut length = 0_usize;
        let mut minted = 0_usize;
        for round in 0..DIFFERENTIAL_ROUNDS {
            let replica = rng.below(3) as u64 + 1;
            let pos = rng.below(length + 1);
            let budget = minted < DIFFERENTIAL_MAX_NODES;
            let text = match rng.below(10) {
                // A paste whose length straddles the cap.
                0..=2 if budget => cap_text(1 + rng.below(MAX_RUN_LEN + MAX_RUN_LEN / 2)),
                3..=6 if budget => cap_text(1 + rng.below(4)),
                choice => {
                    let span = if choice == 7 { 1 } else { 1 + rng.below(64) };
                    blocked.delete_range(pos, pos + span).unwrap();
                    simple.delete_range(pos, pos + span).unwrap();
                    String::new()
                }
            };
            if !text.is_empty() {
                blocked
                    .insert_str_with_replica(pos, replica, &text)
                    .unwrap();
                simple.insert_str_with_replica(pos, replica, &text).unwrap();
                minted += text.chars().count();
            }
            let observed = blocked.get_text().unwrap();
            assert_eq!(
                observed,
                simple.get_text().unwrap(),
                "round {round} of seed {DIFFERENTIAL_SEED:#x} diverged from the \
                 blockless control"
            );
            length = observed.chars().count();
        }
        assert!(
            minted > 2 * MAX_RUN_LEN,
            "the sweep never pasted past the cap: {minted} characters minted"
        );
    }
}

/// Differential and exhaustive verification against `fugue.rs` as the model.
///
/// `fugue.rs` is Algorithm 1 of the paper, unaware of storage, blocks,
/// coalescing or merging, so running one op script through both and comparing
/// text tests everything this module adds rather than restating it. Scripts mix
/// coalescing, mid-run inserts and deletes, since only their combination
/// exercises the block layer.
#[cfg(test)]
mod model_tests {
    use std::collections::BTreeMap;

    use super::{FugueText, TextBlock, UnorderedMap};
    use crate::collections::fugue::{FugueNode, FugueTree, Rng};
    use crate::env;
    use crate::store::{MockedStorage, StorageAdaptor};

    /// The oracle: Algorithm 1, with the same id allocation `FugueText` uses.
    /// A replica's counter is the number of nodes it has minted, which is what
    /// `next_counter` derives from the block set, since a replica's runs always
    /// cover `0..N` contiguously.
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
    /// runs coalesce; a `pos` in the middle mints a separate run.
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

    /// The op alphabet of the exhaustive sweep: one appending insert (which
    /// coalesces), two mid-document inserts (which do not), and two deletes.
    pub(super) const ALPHABET: [Op; 5] = [
        Op::Ins(usize::MAX, "x"),
        Op::Ins(1, "y"),
        Op::Ins(2, "zz"),
        Op::Del(1, 1),
        Op::Del(0, 3),
    ];

    /// EXHAUSTIVE, not sampled: every pair of ops on each of two replicas, so
    /// 5^2 x 5^2 = 625 scripts, each merged in both orders against the model.
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
    /// Merge relies on it, since `merged != mine` decides whether to write.
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

/// The positional read API, asserted against the observable contract
/// (`text_range` / `char_at` agree with `get_text`, and the stored form covers
/// every authoritative node once) rather than against any internal layout.
#[cfg(test)]
mod positional_read_tests {
    use super::{build_tree, find_block, BlockId, FugueText, UnorderedMap};
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
    /// inserts (which do not) and deletes, covering every storage shape.
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
    /// instead of erroring, which is what a concurrent remote delete needs.
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

    /// Positions are `char` indices, not bytes.
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
    /// This is what lets [`find_block`] be a binary search: a replica's runs
    /// PARTITION its counter space. Reintroduce overlapping runs and this goes
    /// red, rather than `find_block` silently picking one of several covers.
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

            // The partition: same-replica runs never overlap.
            for (position, earlier) in loaded.iter().enumerate() {
                for later in loaded.iter().skip(position + 1) {
                    if earlier.id.replica == later.id.replica {
                        assert!(
                            u64::from(earlier.id.counter) + earlier.len as u64
                                <= u64::from(later.id.counter),
                            "seed {seed}: runs {:?}+{} and {:?} overlap",
                            earlier.id,
                            earlier.len,
                            later.id
                        );
                    }
                }
            }

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

/// Convergence through the REAL receive path, [`Interface::apply_action`],
/// rather than through [`FugueText::merge_blocks_from`].
///
/// The join is correct, but it is not what a node runs when a delta lands off
/// the wire: an entry element is created untagged, so without the per-entry
/// `CrdtType::FugueTextBlock` stamp a value collision on one `BlockKey` reaches
/// `Interface::try_merge_non_root` with `crdt_type: None` and resolves
/// last-writer-wins. Such collisions are ordinary here, one writer typing at the
/// end of a run while another deletes inside it.
#[cfg(test)]
mod apply_path_tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::model_tests::{Model, Op, ALPHABET};
    use super::{BlockId, BlockKey, FugueText, TextBlock, MAX_RUN_LEN};
    use crate::collections::fugue::Rng;
    use crate::collections::{CrdtType, Root};
    use crate::delta::{clear_pending_delta, StorageDelta};
    use crate::env::{self, RuntimeEnv};
    use crate::index::Index;
    use crate::interface::{ApplyContext, Interface};
    use crate::store::{Key, MainStorage};

    /// An in-memory main-storage backend owned by one replica. Same shape as
    /// `crate::testing`'s, kept local so these tests need no feature flag.
    type Store = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;

    /// Must be the native default context: `collections::ROOT_ID` is a
    /// process-global `LazyLock` seeded from the first `context_id()` anything
    /// in the binary asks for, so a different id here breaks `Root::new` for
    /// every other test in the process.
    const CONTEXT_ID: [u8; 32] = [236_u8; 32];

    /// Both replicas must derive the SAME collection id, or their actions build
    /// two parallel documents that never meet.
    const FIELD: &str = "apply_path_doc";

    const CAP_SPAN: usize = MAX_RUN_LEN + 3; // both replicas cross the cap before syncing
    const PASTE_LEN: usize = 600; // past two run caps, so a paste spans several blocks
    const PASTE_BYTES_PER_CHAR: usize = 8; // a per-character path blows straight past this
    const EQUIV_SEED: u64 = 0x_ba_7c_1e_d0;
    const EQUIV_ROUNDS: usize = 40;
    const EQUIV_MAX_NODES: usize = 1_400; // the per-character oracle is quadratic in this
    const EQUIV_POOL: [char; 4] = ['a', 'é', '日', 'Z']; // mixed widths: bytes != nodes
    /// Paste lengths the sweep draws from: under, at and several times over the
    /// run cap, plus the empty string and a single character.
    const EQUIV_LENGTHS: [usize; 7] = [
        0,
        1,
        3,
        MAX_RUN_LEN - 1,
        MAX_RUN_LEN,
        MAX_RUN_LEN + 1,
        3 * MAX_RUN_LEN + 5,
    ];

    fn new_store() -> Store {
        Rc::new(RefCell::new(HashMap::new()))
    }

    /// A [`RuntimeEnv`] routing all `MainStorage` I/O into `store`. The device
    /// id is what `local_replica` derives a replica id from, so it is what makes
    /// two replicas mint distinct nodes.
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
    /// sender's root entry, which is not the receiver's to overwrite.
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

    /// The stored block set, borsh-encoded. Two replicas agreeing on the text
    /// but not on these bytes still hash differently and never converge.
    fn blocks_in(store: &Store, device: [u8; 32]) -> Vec<u8> {
        env::with_runtime_env(env_for(store, device), || {
            let doc = Root::<FugueText<MainStorage>>::fetch().expect("document root should exist");
            let mut blocks: Vec<(BlockId, TextBlock)> = doc
                .blocks
                .entries()
                .expect("entries should succeed")
                .map(|(key, block)| (key.id(), block))
                .collect();
            blocks.sort_by_key(|(id, _)| *id);
            borsh::to_vec(&blocks).expect("blocks should serialize")
        })
    }

    /// Two replicas holding EQUAL-LENGTH, DIFFERENT text for one block key,
    /// reachable when a replica id is reused over counter space already spent.
    /// Neither copy is a prefix of the other, so the join must pick one by a
    /// total order; "the longer one" alone leaves each side on its own copy.
    #[test]
    fn equal_length_different_text_for_one_key_converges_on_the_apply_path() {
        let (dev_a, dev_b) = (device(1), device(2));

        let genesis = new_store();
        clear_pending_delta();
        env::with_runtime_env(env_for(&genesis, dev_a), || {
            let mut doc = Root::new(|| FugueText::<MainStorage>::new_with_field_name(FIELD));
            doc.insert_str_with_replica(0, 0, "S")
                .expect("seed insert should succeed");
            doc.commit();
            let _ignored = env::take_last_artifact();
        });

        let store_a = genesis;
        let store_b: Store = Rc::new(RefCell::new(store_a.borrow().clone()));

        // Both writers mint (7, 0) and (7, 1), so one key ends up with two
        // two-character values that share no prefix.
        let delta_a = edit(&store_a, dev_a, |doc| {
            doc.insert_str_with_replica(1, 7, "ab")
                .expect("insert should succeed");
        });
        let delta_b = edit(&store_b, dev_b, |doc| {
            doc.insert_str_with_replica(1, 7, "cd")
                .expect("insert should succeed");
        });

        land(&store_a, dev_a, &delta_b);
        land(&store_b, dev_b, &delta_a);

        assert_eq!(
            text_in(&store_a, dev_a),
            "Scd",
            "replica A did not converge"
        );
        assert_eq!(
            text_in(&store_b, dev_b),
            "Scd",
            "replica B did not converge"
        );
        assert_eq!(
            blocks_in(&store_a, dev_a),
            blocks_in(&store_b, dev_b),
            "the replicas read the same text from different stored bytes, so their \
             Merkle hashes still differ"
        );
    }

    /// One writer typing at the END of a run, which coalesces and rewrites key
    /// `(A,0)` as `"abc"`, while another deletes inside it, which rewrites that
    /// same key's tombstone bitmap. Both replicas then hold a different value
    /// for one key.
    ///
    /// [`join_block`] ORs the bitmaps and takes the longer text, giving `"ac"`.
    /// An untagged LWW resolution erases whichever side loses instead, and both
    /// replicas still AGREE either way, so only the merged VALUE catches it.
    #[test]
    fn coalesce_meeting_a_delete_on_the_apply_path_keeps_every_node() {
        // Deliberately NOT `env::reset_environment()`: it clears PROCESS-global
        // mocked state that other tests in this binary use concurrently.
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

    /// How many block rows the document occupies in `store`.
    fn block_count(store: &Store, device: [u8; 32]) -> usize {
        env::with_runtime_env(env_for(store, device), || {
            Root::<FugueText<MainStorage>>::fetch()
                .expect("document root should exist")
                .blocks
                .len()
                .expect("len should succeed")
        })
    }

    /// Two replicas each typing a run past the cap, reconciled through the real
    /// apply path: same text, same stored bytes, and neither typed passage
    /// shredded by the extra block boundary.
    #[test]
    fn typing_across_the_run_cap_converges_on_the_apply_path() {
        let (dev_a, dev_b) = (device(1), device(2));

        let genesis = new_store();
        clear_pending_delta();
        env::with_runtime_env(env_for(&genesis, dev_a), || {
            let mut doc = Root::new(|| FugueText::<MainStorage>::new_with_field_name(FIELD));
            doc.insert_str_with_replica(0, 0, "S")
                .expect("seed insert should succeed");
            doc.commit();
            let _ignored = env::take_last_artifact();
        });

        let store_a = genesis;
        let store_b: Store = Rc::new(RefCell::new(store_a.borrow().clone()));

        let type_run = |store: &Store, device: [u8; 32], content: char| -> Vec<Vec<u8>> {
            (0..CAP_SPAN)
                .map(|i| {
                    edit(store, device, |doc| {
                        doc.insert(1 + i, content).expect("append should succeed");
                    })
                })
                .collect()
        };
        let deltas_a = type_run(&store_a, dev_a, 'a');
        let deltas_b = type_run(&store_b, dev_b, 'b');

        for delta in &deltas_b {
            land(&store_a, dev_a, delta);
        }
        for delta in &deltas_a {
            land(&store_b, dev_b, delta);
        }

        let text = text_in(&store_a, dev_a);
        assert_eq!(text, text_in(&store_b, dev_b), "the replicas disagree");
        assert_eq!(
            blocks_in(&store_a, dev_a),
            blocks_in(&store_b, dev_b),
            "the replicas read the same text from different stored bytes, so their \
             Merkle hashes still differ"
        );
        assert_eq!(text.chars().count(), 1 + 2 * CAP_SPAN);
        assert!(
            text.contains(&"a".repeat(CAP_SPAN)) && text.contains(&"b".repeat(CAP_SPAN)),
            "a typed passage was shredded across the cap boundary: {text}"
        );
        // The seed block, plus a full block and a remainder from each replica.
        assert_eq!(block_count(&store_a, dev_a), 5);
    }

    /// Run one two-replica scenario end to end through the APPLY path, and check
    /// both replicas against the model oracle.
    ///
    /// The counterpart of `model_tests::scenario`, which reconciles through
    /// `merge_blocks_from`: identical script and oracle, but the reconciliation
    /// path a node actually runs.
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

    /// EXHAUSTIVE over one op per replica: all 5 x 5 = 25 pairs of the model
    /// sweep's alphabet, reconciled through `apply_action`. One op per side is
    /// where the coalesce-versus-delete collision lives, so this covers the
    /// failure class exhaustively rather than by sampling.
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

    /// One step of the batched-versus-per-character sweep.
    #[derive(Debug)]
    enum Step {
        Insert {
            pos: usize,
            replica: u64,
            text: String,
        },
        Delete {
            start: usize,
            span: usize,
        },
    }

    /// A random edit script.
    ///
    /// The document length is tracked arithmetically rather than read back, so
    /// the script is a pure function of the seed and both paths replay the same
    /// one. Positions are drawn as start / middle / end / free, the four the
    /// Fugue insert rule resolves differently.
    fn equivalence_script() -> Vec<Step> {
        let mut rng = Rng::new(EQUIV_SEED);
        let mut script = Vec::with_capacity(EQUIV_ROUNDS);
        let mut len = 0_usize;
        let mut minted = 0_usize;

        for round in 0..EQUIV_ROUNDS {
            if round > 0 && rng.below(4) == 0 {
                let start = rng.below(len + 1);
                let span = 1 + rng.below(8);
                len -= (start + span).min(len).saturating_sub(start);
                script.push(Step::Delete { start, span });
                continue;
            }
            let drawn = EQUIV_LENGTHS[rng.below(EQUIV_LENGTHS.len())];
            let count = if minted + drawn > EQUIV_MAX_NODES {
                rng.below(4)
            } else {
                drawn
            };
            let pos = match rng.below(4) {
                0 => 0,
                1 => len / 2,
                2 => len,
                _ => rng.below(len + 1),
            };
            script.push(Step::Insert {
                pos,
                replica: 1 + rng.below(3) as u64,
                text: (0..count)
                    .map(|_| EQUIV_POOL[rng.below(EQUIV_POOL.len())])
                    .collect(),
            });
            minted += count;
            len += count;
        }
        script
    }

    /// Everything two replicas must agree on: the block rows, the CRDT tag on
    /// each row, and the Merkle root. The rows are carried alongside the root so
    /// a mismatch names the block instead of dumping the whole set.
    type StoredState = (
        Vec<(BlockId, TextBlock)>,
        Vec<Option<CrdtType>>,
        Option<[u8; 32]>,
    );

    /// Drive `script` into a fresh store and read its stored state back.
    fn replay(script: &[Step], per_char: bool) -> StoredState {
        let store = new_store();
        let dev = device(1);
        clear_pending_delta();
        let root = env::with_runtime_env(env_for(&store, dev), || {
            let mut doc = Root::new(|| FugueText::<MainStorage>::new_with_field_name(FIELD));
            for step in script {
                match *step {
                    Step::Insert {
                        pos,
                        replica,
                        ref text,
                    } => {
                        if per_char {
                            doc.insert_str_per_char(pos, replica, text)
                        } else {
                            doc.insert_str_with_replica(pos, replica, text)
                        }
                        .expect("insert should succeed");
                    }
                    Step::Delete { start, span } => {
                        doc.delete_range(start, start + span)
                            .expect("delete should succeed");
                    }
                }
            }
            doc.commit();
            let _ignored = env::take_last_artifact();
            env::root_hash()
        });

        let (blocks, tags) = env::with_runtime_env(env_for(&store, dev), || {
            let doc = Root::<FugueText<MainStorage>>::fetch().expect("document root should exist");
            let mut blocks: Vec<(BlockId, TextBlock)> = doc
                .blocks
                .entries()
                .expect("entries should succeed")
                .map(|(key, block)| (key.id(), block))
                .collect();
            blocks.sort_by_key(|(id, _)| *id);
            let tags = blocks
                .iter()
                .map(|(id, _)| {
                    Index::<MainStorage>::get_index(doc.blocks.entry_id(&BlockKey::new(*id)))
                        .expect("index read should succeed")
                        .and_then(|index| index.metadata.crdt_type)
                })
                .collect();
            (blocks, tags)
        });
        (blocks, tags, root)
    }

    /// A multi-character insert must store exactly what the per-character loop
    /// stored: same block keys, same bytes, same CRDT tag, same Merkle root.
    /// Anything else forks state against every replica that has not upgraded.
    #[test]
    fn batched_insert_str_stores_what_the_per_character_loop_stored() {
        let script = equivalence_script();

        // The sweep is an oracle only if the seed actually drew every shape.
        let pastes: Vec<usize> = script
            .iter()
            .filter_map(|step| match *step {
                Step::Insert { ref text, .. } => Some(text.chars().count()),
                Step::Delete { .. } => None,
            })
            .collect();
        for (what, drawn) in [
            ("an empty string", pastes.contains(&0)),
            ("a paste under the cap", pastes.contains(&3)),
            ("a paste at the cap", pastes.contains(&MAX_RUN_LEN)),
            (
                "a paste several caps long",
                pastes.iter().any(|n| *n > 2 * MAX_RUN_LEN),
            ),
            ("a delete", pastes.len() < script.len()),
            (
                "an insert at the start",
                script
                    .iter()
                    .any(|step| matches!(*step, Step::Insert { pos: 0, .. })),
            ),
        ] {
            assert!(drawn, "seed {EQUIV_SEED:#x} never drew {what}");
        }

        let (want, want_tags, want_root) = replay(&script, true);
        let (got, got_tags, got_root) = replay(&script, false);

        let ids = |blocks: &[(BlockId, TextBlock)]| -> Vec<BlockId> {
            blocks.iter().map(|(id, _)| *id).collect()
        };
        assert_eq!(
            ids(&got),
            ids(&want),
            "seed {EQUIV_SEED:#x}: the batched path stored a different block set"
        );
        for ((id, block), (_, expected)) in got.iter().zip(&want) {
            assert_eq!(
                block, expected,
                "seed {EQUIV_SEED:#x}: block {id:?} differs from the per-character loop's"
            );
        }
        assert_eq!(
            got_tags, want_tags,
            "seed {EQUIV_SEED:#x}: CRDT tags differ"
        );
        assert_eq!(
            got_root, want_root,
            "seed {EQUIV_SEED:#x}: Merkle roots differ"
        );
    }

    /// A paste costs one action per block it touches, not one per character: a
    /// per-character path re-writes the coalescing run on every character, so
    /// the delta carries a copy of the growing run for each of them.
    #[test]
    fn one_insert_str_emits_one_action_per_block_it_touches() {
        let store = new_store();
        let dev = device(1);
        clear_pending_delta();
        env::with_runtime_env(env_for(&store, dev), || {
            Root::new(|| FugueText::<MainStorage>::new_with_field_name(FIELD)).commit();
            let _ignored = env::take_last_artifact();
        });

        let text: String = std::iter::repeat_n('a', PASTE_LEN).collect();
        let delta = edit(&store, dev, |doc| {
            doc.insert_str(0, &text).expect("insert_str should succeed");
        });
        let actions = match borsh::from_slice::<StorageDelta>(&delta).expect("delta should decode")
        {
            StorageDelta::Actions(actions) => actions,
            StorageDelta::CausalActions { actions, .. } => actions,
        };

        // One per block, plus the block collection's own element.
        let blocks = PASTE_LEN.div_ceil(MAX_RUN_LEN);
        let written = actions.iter().filter(|a| !a.id().is_root()).count();
        assert_eq!(
            written,
            blocks + 1,
            "{written} non-root actions for a paste spanning {blocks} blocks"
        );
        assert!(
            delta.len() <= PASTE_LEN * PASTE_BYTES_PER_CHAR,
            "{} delta bytes for {PASTE_LEN} pasted characters",
            delta.len()
        );
    }
}
