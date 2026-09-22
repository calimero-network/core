//! `FugueText` - storage for the Tree-Fugue algorithm in [`fugue`](super::fugue).
//!
//! One entity is one run of up to `MAX_RUN_LEN` nodes, each after the first the right
//! child of the one before; a full run is never rewritten. Order is recomputed from the
//! stored blocks through a [`FugueTree`] on every call, so gas is equal on every replica.
//!
//! # The unit of a position
//!
//! Every `pos`, `start`, `end` and `TextOp` count here is an index into Unicode
//! SCALAR VALUES (Rust `char`), never bytes and never UTF-16 code units. That covers
//! `insert`, `insert_str`, `insert_str_with_replica`, `delete`, `delete_range`,
//! `text_range`, `char_at`, `anchor_at`, `len` and every `TextOp` an `apply_delta`
//! carries. An astral character is one position and a combining mark is its own,
//! so a grapheme cluster spans several. A browser counts UTF-16 code units, where
//! every astral character is two, so a TypeScript binding converts on both edges.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use super::fugue::{FugueNode, FugueTree, NodeId, RawId, Side};
use super::{CrdtType, UnorderedMap};
use crate::collections::error::StoreError;
use crate::env;
use crate::store::{MainStorage, StorageAdaptor};

const MAX_RUN_LEN: usize = 256; // nodes per block
const COUNTER_EXHAUSTED: &str = "replica counter space exhausted";

/// The run's FIRST node: it covers `counter..counter + len` of `replica`'s counter space.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
pub(crate) struct BlockId {
    replica: u64,
    counter: u32,
}

impl BlockId {
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
        // `BlockId` is fixed-size POD, so serialization cannot fail.
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

/// Borsh-serializable mirror of [`Side`], which `fugue.rs` keeps storage-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub(crate) enum BlockSide {
    L,
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
    /// Duplicated from the map key so the stored value is self-describing.
    start_id: BlockId,
    /// Node `i` of the run holds the `i`-th character.
    text: String,
    /// The parent of the run's FIRST node; `None` is the Fugue root.
    parent: Option<BlockId>,
    side: BlockSide,
    /// Per NODE: a run GROWS by coalescing, so a per-run flag could not say which nodes it covered.
    tombstones: Vec<u8>,
}

impl TextBlock {
    /// Nodes, not bytes.
    fn len(&self) -> usize {
        self.text.chars().count()
    }
}

fn tomb_get(bits: &[u8], offset: usize) -> bool {
    bits.get(offset / 8)
        .is_some_and(|byte| byte & (1_u8 << (offset % 8)) != 0)
}

fn tomb_set(bits: &mut Vec<u8>, offset: usize) {
    let byte = offset / 8;
    if bits.len() <= byte {
        bits.resize(byte + 1, 0);
    }
    if let Some(slot) = bits.get_mut(byte) {
        *slot |= 1_u8 << (offset % 8);
    }
}

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

/// Which side of its character an [`Anchor`] sits on. JSON: `"Before"` / `"After"`.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub enum Bias {
    Before,
    After,
}

/// A cursor that survives concurrent edits: the gap beside a character, or a document edge.
///
/// JSON: `"Start"`, `"End"`, or `{"Char":{"id":[replica,counter],"bias":"After"}}`.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub enum Anchor {
    Start,
    End,
    Char { id: RawId, bias: Bias },
}

/// The ids one insert minted: `len` consecutive counters from `start`.
/// JSON: `{"start":[replica,counter],"len":n}`.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct IdRange {
    pub start: RawId,
    pub len: u32,
}

/// What a delete took out, and where from: the input to [`FugueText::insert_str_at`].
/// JSON: `{"text":"...","anchor":<Anchor>,"ids":[<IdRange>]}`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Removed {
    pub text: String,
    pub anchor: Anchor,
    /// The characters taken, as runs of consecutive ids: a delete can span several.
    pub ids: Vec<IdRange>,
}

/// One step of an editor change, walking the document as it was before the change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextOp {
    Retain(usize),
    Insert(String),
    Delete(usize),
}

/// What one op of an applied change took, so the change can be taken back.
/// JSON: `{"Inserted":<IdRange>}` or `{"Removed":<Removed>}`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum Undo {
    Inserted(IdRange),
    Removed(Removed),
}

/// A storage-backed collaborative text collection with Tree-Fugue ordering.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct FugueText<S: StorageAdaptor = MainStorage> {
    #[borsh(bound(serialize = "", deserialize = ""))]
    pub(crate) blocks: UnorderedMap<BlockKey, TextBlock, S>,
}

/// Re-key the block map under its storage parent so a nested `FugueText` converges.
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
    pub(super) fn new_internal() -> Self {
        Self {
            blocks: UnorderedMap::new_internal(),
        }
    }

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

    /// Called by the `#[app::state]` macro after `init()`.
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        self.blocks.reassign_deterministic_id(field_name);
        self.blocks.set_collection_crdt_type(CrdtType::FugueText);
    }

    /// Insert a character at `pos`. Panics inside a state migration.
    pub fn insert(&mut self, pos: usize, content: char) -> Result<(), StoreError> {
        self.insert_str(pos, content.encode_utf8(&mut [0_u8; 4]))
            .map(drop)
    }

    /// Insert a string at `pos`. Panics inside a state migration.
    pub fn insert_str(&mut self, pos: usize, s: &str) -> Result<Option<IdRange>, StoreError> {
        self.insert_str_with_replica(pos, minting_replica("insert_str"), s)
    }

    /// Insert at `pos` under an explicit `replica`; two writers sharing one mint colliding ids.
    pub fn insert_str_with_replica(
        &mut self,
        pos: usize,
        replica: u64,
        s: &str,
    ) -> Result<Option<IdRange>, StoreError> {
        // Re-key thunk for a document stored as a collection value.
        let _ignored = super::rekey::register_rekey::<Self>();

        let mut draft = Draft::open(self)?;
        let minted = draft.insert(Place::Index(pos), replica, s, false)?;
        self.flush(draft)?;
        Ok(minted)
    }

    /// Insert directly after `left` in DOCUMENT order, tombstones included.
    ///
    /// The only way to place a character among tombstones, which is what
    /// Peritext's formatting-boundary rule needs and a visible index cannot say.
    pub(super) fn insert_str_after(
        &mut self,
        left: NodeId,
        replica: u64,
        s: &str,
    ) -> Result<Option<IdRange>, StoreError> {
        let _ignored = super::rekey::register_rekey::<Self>();

        let mut draft = Draft::open(self)?;
        let minted = draft.insert(Place::After(left), replica, s, false)?;
        self.flush(draft)?;
        Ok(minted)
    }

    /// Insert at the gap `anchor` names now: the undo of a delete. Panics inside a state migration.
    pub fn insert_str_at(
        &mut self,
        anchor: &Anchor,
        s: &str,
    ) -> Result<Option<IdRange>, StoreError> {
        let replica = minting_replica("insert_str_at");
        let _ignored = super::rekey::register_rekey::<Self>();

        let mut draft = Draft::open(self)?;
        let pos = resolve_in(&draft.tree, anchor)?;
        let minted = draft.insert(Place::Index(pos), replica, s, false)?;
        self.flush(draft)?;
        Ok(minted)
    }

    /// Delete the characters `ids` names that are still live: the undo of an insert.
    pub fn delete_ids(&mut self, ids: &IdRange) -> Result<Option<Removed>, StoreError> {
        let mut draft = Draft::open(self)?;
        let end = u64::from(ids.start.1) + u64::from(ids.len);
        let picked = draft
            .tree
            .delete_ids_in(&draft.order, |(replica, counter)| {
                replica == ids.start.0 && counter >= ids.start.1 && u64::from(counter) < end
            });
        let removed = draft.bury(&picked)?;
        self.flush(draft)?;
        Ok(removed)
    }

    /// Apply a whole editor change in one call, returning what [`Self::undo`]
    /// takes to reverse it, in application order. Panics inside a state migration.
    pub fn apply_delta(&mut self, ops: &[TextOp]) -> Result<Vec<Undo>, StoreError> {
        let replica = minting_replica("apply_delta");
        let _ignored = super::rekey::register_rekey::<Self>();

        let mut draft = Draft::open(self)?;
        let mut steps = Vec::new();
        let mut pos = 0_usize;
        for (index, op) in ops.iter().enumerate() {
            match *op {
                TextOp::Retain(count) => pos = advance(pos, count)?,
                TextOp::Insert(ref text) => {
                    let minted =
                        draft.insert(Place::Index(pos), replica, text, index + 1 < ops.len())?;
                    steps.extend(minted.map(Undo::Inserted));
                    pos = advance(pos, text.chars().count())?;
                }
                TextOp::Delete(count) => {
                    let removed = draft.delete(pos, advance(pos, count)?)?;
                    steps.extend(removed.map(Undo::Removed));
                }
            }
        }
        self.flush(draft)?;
        Ok(steps)
    }

    /// Reverse `steps`, last one first, returning what redoes them. Each step is
    /// its own write, so a failure part-way leaves the earlier ones undone.
    pub fn undo(&mut self, steps: &[Undo]) -> Result<Vec<Undo>, StoreError> {
        let mut redo = Vec::with_capacity(steps.len());
        for step in steps.iter().rev() {
            match *step {
                Undo::Inserted(ref minted) => {
                    redo.extend(self.delete_ids(minted)?.map(Undo::Removed));
                }
                Undo::Removed(ref removed) => {
                    redo.extend(
                        self.insert_str_at(&removed.anchor, &removed.text)?
                            .map(Undo::Inserted),
                    );
                }
            }
        }
        Ok(redo)
    }

    /// Delete the character at `pos`.
    pub fn delete(&mut self, pos: usize) -> Result<(), StoreError> {
        self.delete_range(pos, pos.checked_add(1).ok_or_else(|| out_of_bounds(pos))?)
            .map(drop)
    }

    /// Delete the half-open range `start..end` of visible positions, clamped in `end`.
    pub fn delete_range(
        &mut self,
        start: usize,
        end: usize,
    ) -> Result<Option<Removed>, StoreError> {
        if start > end {
            return Err(invalid("start must be <= end"));
        }
        let mut draft = Draft::open(self)?;
        let removed = draft.delete(start, end)?;
        self.flush(draft)?;
        Ok(removed)
    }

    /// The document text, tombstones excluded.
    pub fn get_text(&self) -> Result<String, StoreError> {
        let loaded = self.load()?;
        Ok(build_tree(&loaded)?.values())
    }

    /// The characters in `start..end`, by char index, clamped in `end`.
    pub fn text_range(&self, start: usize, end: usize) -> Result<String, StoreError> {
        if start > end {
            return Err(invalid("start must be <= end"));
        }
        let loaded = self.load()?;
        let text = build_tree(&loaded)?.values();
        Ok(text.chars().skip(start).take(end - start).collect())
    }

    /// The character at `pos`, a char index, or `None` past the end.
    pub fn char_at(&self, pos: usize) -> Result<Option<char>, StoreError> {
        let loaded = self.load()?;
        Ok(build_tree(&loaded)?.values().chars().nth(pos))
    }

    /// The anchor for the gap at `pos`: `After` holds the character on its left, `Before` its right.
    pub fn anchor_at(&self, pos: usize, bias: Bias) -> Result<Anchor, StoreError> {
        anchor_at_in(&self.tree()?, pos, bias)
    }

    /// The gap `anchor` names now; a deleted character resolves to the gap it left.
    pub fn resolve(&self, anchor: &Anchor) -> Result<usize, StoreError> {
        resolve_in(&self.tree()?, anchor)
    }

    /// [`Self::resolve`] for many anchors against ONE rebuild of the tree, which is
    /// what makes rendering a document's cursors and marks affordable.
    pub fn resolve_many(&self, anchors: &[Anchor]) -> Result<Vec<usize>, StoreError> {
        let index = PositionIndex::build(&self.tree()?);
        anchors
            .iter()
            .map(|anchor| index.resolve(anchor).ok_or_else(unknown_anchor))
            .collect()
    }

    /// The stored blocks expanded into one tree: the single rebuild every read owes.
    pub(super) fn tree(&self) -> Result<FugueTree, StoreError> {
        build_tree(&self.load()?)
    }

    /// The number of visible characters.
    pub fn len(&self) -> Result<usize, StoreError> {
        let loaded = self.load()?;
        Ok(build_tree(&loaded)?.len())
    }

    /// Whether the document has no visible characters.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        self.len().map(|len| len == 0)
    }

    /// The equivalence oracle for the batched path.
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

    fn flush(&mut self, draft: Draft) -> Result<(), StoreError> {
        for lb in draft.loaded {
            if draft.dirty.contains(&lb.id) {
                self.put_block(BlockKey::new(lb.id), lb.block)?;
            }
        }
        Ok(())
    }

    /// A block is MUTABLE under one key, so an untagged entity would reconcile last-writer-wins.
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

    /// Copy in `other`'s blocks, joined per key by [`join_block`]; a removed block stays removed.
    pub(crate) fn merge_blocks_from<S2: StorageAdaptor>(
        &mut self,
        other: &FugueText<S2>,
    ) -> Result<(), StoreError> {
        for (key, incoming) in other.blocks.entries()? {
            // A read error must propagate: treating it as absent would re-insert over live data.
            if let Some(mine) = self.blocks.get(&key)? {
                let mine = mine.into_inner();
                let mut merged = mine.clone();
                join_block(&mut merged, incoming);
                if merged != mine {
                    self.put_block(key, merged)?;
                }
                continue;
            }

            // A storage tombstone means `self` deleted it: delete wins, do not resurrect.
            if crate::index::Index::<S>::is_deleted(self.blocks.entry_id(&key))? {
                continue;
            }
            self.put_block(key, incoming)?;
        }
        Ok(())
    }

    /// The leaf-merge entry point the SYNC path reaches through `merge_by_crdt_type`.
    pub(crate) fn merge_block_entry_bytes(
        existing: &[u8],
        incoming: &[u8],
    ) -> Result<Vec<u8>, super::crdt_meta::MergeError> {
        use super::crdt_meta::MergeError;

        // `(value, key)`: the reverse order still decodes, so getting it wrong is a silent bad join.
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

/// Tombstone OR plus the max of `(node count, text, parent, side)`: convergent for hostile peers.
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

/// Where an insert hangs: a visible index, or the node it must directly follow.
enum Place {
    Index(usize),
    After(NodeId),
}

/// A stored block plus its node count.
struct LoadedBlock {
    id: BlockId,
    block: TextBlock,
    len: usize,
}

/// One call's blocks and tree, edited in memory so each touched block is written once.
struct Draft {
    loaded: Vec<LoadedBlock>,
    tree: FugueTree,
    order: Vec<NodeId>,
    dirty: BTreeSet<BlockId>,
}

impl Draft {
    fn open<S: StorageAdaptor>(doc: &FugueText<S>) -> Result<Self, StoreError> {
        let loaded = doc.load()?;
        let tree = build_tree(&loaded)?;
        Ok(Self {
            order: tree.order(),
            loaded,
            tree,
            dirty: BTreeSet::new(),
        })
    }

    /// Only the first character needs the insert rule; the rest chain right.
    /// `chain` also grows the tree, which only a later op in the same call reads.
    fn insert(
        &mut self,
        at: Place,
        replica: u64,
        s: &str,
        chain: bool,
    ) -> Result<Option<IdRange>, StoreError> {
        let mut rest = s.chars();
        let Some(first) = rest.next() else {
            return Ok(None);
        };
        let counter = next_counter(replica, &self.loaded)?;
        let id = (replica, counter);
        let node = match at {
            Place::Index(pos) => self.tree.insert_in(&mut self.order, pos, first, id),
            Place::After(left) => self.tree.insert_after_in(&mut self.order, left, first, id),
        }
        .map_err(|err| invalid(&err.to_string()))?;

        let id = BlockId::from_raw(node.id);
        let mut at = match coalesce_target(&self.loaded, node, id) {
            Some(index) => index,
            None => self.open_block(id, node.parent.map(BlockId::from_raw), node.side.into()),
        };
        self.push(at, first);

        let mut chained: Vec<NodeId> = Vec::new();
        let mut last = counter;
        for content in rest {
            let next = bump(last)?;
            if self.loaded[at].len == MAX_RUN_LEN {
                // Past the cap: parent the new block on the full run's last node, side right.
                let (start, parent) = (
                    BlockId {
                        replica,
                        counter: next,
                    },
                    BlockId {
                        replica,
                        counter: last,
                    },
                );
                at = self.open_block(start, Some(parent), BlockSide::R);
            }
            self.push(at, content);
            if chain {
                self.tree.integrate(FugueNode {
                    id: (replica, next),
                    value: Some(content),
                    parent: Some((replica, last)),
                    side: Side::R,
                });
                chained.push(Some((replica, next)));
            }
            last = next;
        }
        // Each chained node is the only right child of the one before it, so they follow `first`.
        if let Some(head) = self.order.iter().position(|n| *n == Some(node.id)) {
            let _ignored = self.order.splice(head + 1..head + 1, chained);
        }
        Ok(Some(IdRange {
            start: node.id,
            len: last - counter + 1,
        }))
    }

    fn open_block(&mut self, id: BlockId, parent: Option<BlockId>, side: BlockSide) -> usize {
        let at = self.loaded.partition_point(|lb| lb.id < id);
        self.loaded.insert(
            at,
            LoadedBlock {
                id,
                len: 0,
                block: TextBlock {
                    start_id: id,
                    text: String::new(),
                    parent,
                    side,
                    tombstones: Vec::new(),
                },
            },
        );
        at
    }

    /// An appended node is live and a trailing zero bit is implicit, so the bitmap is untouched.
    fn push(&mut self, at: usize, content: char) {
        let lb = &mut self.loaded[at];
        lb.block.text.push(content);
        lb.len += 1;
        let _ignored = self.dirty.insert(lb.id);
    }

    fn delete(&mut self, start: usize, end: usize) -> Result<Option<Removed>, StoreError> {
        let picked = self
            .tree
            .delete_in(&self.order, start, end.saturating_sub(start));
        self.bury(&picked)
    }

    /// Set the stored bit of each character the tree just tombstoned.
    fn bury(&mut self, picked: &[(RawId, char)]) -> Result<Option<Removed>, StoreError> {
        for (raw, _) in picked {
            let id = BlockId::from_raw(*raw);
            let index =
                find_block(&self.loaded, id).ok_or_else(|| invalid("deleted node has no block"))?;
            let lb = &mut self.loaded[index];
            tomb_set(
                &mut lb.block.tombstones,
                (id.counter - lb.id.counter) as usize,
            );
            tomb_trim(&mut lb.block.tombstones);
            let _ignored = self.dirty.insert(lb.id);
        }
        Ok(picked.first().map(|(first, _)| Removed {
            text: picked.iter().map(|(_, content)| *content).collect(),
            anchor: Anchor::Char {
                id: *first,
                bias: Bias::Before,
            },
            ids: id_runs(picked),
        }))
    }
}

/// Runs of consecutive ids, so one delete names what it took without listing every character.
fn id_runs(picked: &[(RawId, char)]) -> Vec<IdRange> {
    let mut runs: Vec<IdRange> = Vec::new();
    for &((replica, counter), _) in picked {
        let extends = runs.last().is_some_and(|last| {
            last.start.0 == replica
                && u64::from(last.start.1) + u64::from(last.len) == u64::from(counter)
        });
        match runs.last_mut() {
            Some(last) if extends => last.len += 1,
            _ => runs.push(IdRange {
                start: (replica, counter),
                len: 1,
            }),
        }
    }
    runs
}

/// The anchor for the gap at `pos` in `tree`, the held-tree form of [`FugueText::anchor_at`].
pub(super) fn anchor_at_in(tree: &FugueTree, pos: usize, bias: Bias) -> Result<Anchor, StoreError> {
    let len = tree.len();
    if pos > len {
        return Err(out_of_bounds(pos));
    }
    let index = match bias {
        Bias::After if pos == 0 => return Ok(Anchor::Start),
        Bias::Before if pos == len => return Ok(Anchor::End),
        Bias::After => pos - 1,
        Bias::Before => pos,
    };
    let id = tree.id_at(index).ok_or_else(|| out_of_bounds(pos))?;
    Ok(Anchor::Char { id, bias })
}

/// Every node's live-predecessor count and liveness, from ONE traversal.
///
/// Resolving `m` anchors one at a time is `O(N * m)`, which at document scale is
/// millions of tree steps inside a gas-metered call; this makes it `O(N + m)`.
pub(super) struct PositionIndex {
    of: BTreeMap<RawId, (usize, bool)>,
    len: usize,
}

impl PositionIndex {
    pub(super) fn build(tree: &FugueTree) -> Self {
        let mut of = BTreeMap::new();
        let mut live = 0_usize;
        for id in tree.order().into_iter().flatten() {
            let alive = tree.node(id).is_some_and(|node| node.value.is_some());
            let _ignored = of.insert(id, (live, alive));
            live += usize::from(alive);
        }
        Self { of, len: live }
    }

    /// [`resolve_in`]'s rule, minus the tree walk. `None` is a character this
    /// replica has not received, which a caller rendering many anchors must
    /// skip rather than fail on.
    pub(super) fn resolve(&self, anchor: &Anchor) -> Option<usize> {
        let (id, bias) = match *anchor {
            Anchor::Start => return Some(0),
            Anchor::End => return Some(self.len),
            Anchor::Char { id, bias } => (id, bias),
        };
        let (before, live) = *self.of.get(&id)?;
        Some(before + usize::from(live && bias == Bias::After))
    }
}

/// The gap `anchor` names in `tree`; a deleted character names the gap it left.
fn resolve_in(tree: &FugueTree, anchor: &Anchor) -> Result<usize, StoreError> {
    let (id, bias) = match *anchor {
        Anchor::Start => return Ok(0),
        Anchor::End => return Ok(tree.len()),
        Anchor::Char { id, bias } => (id, bias),
    };
    let before = tree.live_before(id).ok_or_else(unknown_anchor)?;
    let live = tree.node(id).is_some_and(|node| node.value.is_some());
    Ok(before + usize::from(live && bias == Bias::After))
}

fn unknown_anchor() -> StoreError {
    invalid("anchor names an unknown character")
}

fn advance(pos: usize, count: usize) -> Result<usize, StoreError> {
    pos.checked_add(count).ok_or_else(|| out_of_bounds(pos))
}

/// Stops at [`MAX_RUN_LEN`], past which a full block's text is frozen.
fn coalesces_into(lb: &LoadedBlock, id: BlockId) -> bool {
    lb.len < MAX_RUN_LEN
        && lb.id.replica == id.replica
        && u64::from(lb.id.counter) + lb.len as u64 == u64::from(id.counter)
}

/// Only a RIGHT child off the run's LAST node continues it.
fn coalesce_target(loaded: &[LoadedBlock], node: FugueNode, id: BlockId) -> Option<usize> {
    if node.side != Side::R {
        return None;
    }
    let parent = BlockId::from_raw(node.parent?);
    let index = find_block(loaded, parent)?;
    let lb = loaded.get(index)?;
    let offset = (parent.counter - lb.id.counter) as usize;
    (offset + 1 == lb.len && coalesces_into(lb, id)).then_some(index)
}

fn bump(counter: u32) -> Result<u32, StoreError> {
    counter
        .checked_add(1)
        .ok_or_else(|| invalid(COUNTER_EXHAUSTED))
}

/// At most one block can cover `id`: a replica's runs PARTITION its counter space.
fn find_block(loaded: &[LoadedBlock], id: BlockId) -> Option<usize> {
    let position = loaded.partition_point(|lb| lb.id <= id).checked_sub(1)?;
    let lb = loaded.get(position)?;
    (lb.id.replica == id.replica
        && u64::from(id.counter) < u64::from(lb.id.counter) + lb.len as u64)
        .then_some(position)
}

/// Never reuse a counter: [`FugueTree::integrate`] keeps the FIRST definition of a node.
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

/// Tombstoned nodes are emitted too: they still parent live nodes.
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

/// The local replica, for a call that mints ids; a loud panic beats a silent network divergence.
#[expect(
    clippy::panic,
    reason = "minting from the node-local device id inside a migration diverges ids across nodes"
)]
fn minting_replica(method: &str) -> u64 {
    if env::in_merge_mode() {
        panic!(
            "FugueText::{method}() is non-deterministic during a state migration: it mints node \
             ids from the node-local device id, diverging ids across nodes. Seed with \
             `insert_str_with_replica(pos, replica, s)`."
        );
    }
    local_replica()
}

/// The replica id is the first 8 bytes of the device id: a stamp, not a gate.
pub(super) fn local_replica() -> u64 {
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

/// Two scopes standing in for two replicas must derive the same collection id.
#[cfg(test)]
fn doc_in<S: StorageAdaptor>(field_name: &str) -> FugueText<S> {
    FugueText {
        blocks: UnorderedMap::new_with_field_name_and_crdt_type(
            None,
            field_name,
            CrdtType::FugueText,
        ),
    }
}

#[cfg(test)]
mod tests {
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    use super::model_tests::Model;
    use super::{
        doc_in, join_block, tomb_set, Anchor, BlockId, BlockSide, FugueText, TextBlock, TextOp,
        MAX_RUN_LEN,
    };
    use crate::collections::Root;
    use crate::env;
    use crate::store::{MockedStorage, StorageAdaptor};

    const JOIN_LAW_MAX_NODES: usize = 12; // longest generated run: past one bitmap byte
    const JOIN_LAW_POOL: [char; 5] = ['a', 'b', 'z', 'é', '日']; // mixed widths: bytes != nodes
    const JOIN_LAW_SEED: u64 = 0x_5f_09_3e_10;
    const JOIN_LAW_ROUNDS: usize = 500;
    const CAP_POOL: [char; 4] = ['a', 'é', '日', 'z']; // mixed widths: bytes != nodes
    const DIFFERENTIAL_SEED: u64 = 0x_d1_ff_5e_ed;
    const DIFFERENTIAL_ROUNDS: usize = 200;
    const DIFFERENTIAL_MAX_NODES: usize = 800; // the model is quadratic in this

    /// Sorted by id: the canonical form for equality assertions.
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

    #[test]
    fn insert__mid_run_does_not_split_the_run() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str_with_replica(0, 7, "hello").unwrap();
        assert_eq!(stored(&doc).len(), 1);

        doc.insert_str_with_replica(2, 7, "X").unwrap();
        assert_eq!(doc.get_text().unwrap(), "heXllo");

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

    #[test]
    fn insert__repeated_mid_document_typing_adds_one_block_each() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str_with_replica(0, 7, "abcdef").unwrap();
        assert_eq!(stored(&doc).len(), 1);

        // An ADVANCING caret chains right, so the inserted characters coalesce into one run.
        for offset in 0..5 {
            doc.insert_str_with_replica(3 + offset, 7, "z").unwrap();
        }
        assert_eq!(doc.get_text().unwrap(), "abczzzzzdef");
        assert_eq!(
            stored(&doc).len(),
            2,
            "one untouched run plus one coalesced run of inserted text"
        );

        // A caret that does NOT advance chains left: one run each.
        let mut fixed = Root::new(FugueText::new);
        fixed.insert_str_with_replica(0, 8, "abcdef").unwrap();
        for _ in 0..3 {
            fixed.insert_str_with_replica(3, 8, "z").unwrap();
        }
        assert_eq!(fixed.get_text().unwrap(), "abczzzdef");
        assert_eq!(stored(&fixed).len(), 4);
    }

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

    #[test]
    fn merge__tombstone_of_a_short_copy_does_not_eat_a_coalesced_node() {
        type A = crate::store::MockedStorage<869>;
        type B = crate::store::MockedStorage<870>;
        env::reset_for_testing();

        let mut a = doc_in::<A>("c1");
        a.insert_str_with_replica(0, 2, "P").unwrap();
        let mut b = doc_in::<B>("c1");
        b.insert_str_with_replica(0, 2, "P").unwrap();

        // B coalesces "M" into the SAME entity that A tombstones.
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

    #[test]
    fn merge__coalesced_run_meeting_a_split_copy_loses_no_node() {
        type A = crate::store::MockedStorage<871>;
        type B = crate::store::MockedStorage<872>;
        env::reset_for_testing();

        let mut a = doc_in::<A>("c2");
        a.insert_str_with_replica(0, 2, "ab").unwrap();
        let mut b = doc_in::<B>("c2");
        b.insert_str_with_replica(0, 2, "ab").unwrap();

        // The merged set holds (2,0)"abc" overlapping (2,1)"b".
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

    /// Seeded with the shared `S`, appends `passage`, then inserts `heading` before it.
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

    #[test]
    fn merge__backward_insertion_does_not_interleave() {
        type A = crate::store::MockedStorage<863>;
        type B = crate::store::MockedStorage<864>;
        env::reset_for_testing();

        let alice = backward_insertion_doc::<A>("doc", 1, "A", "aaa");
        let bob = backward_insertion_doc::<B>("doc", 2, "B", "bbb");
        assert_eq!(alice.get_text().unwrap(), "SAaaa");
        assert_eq!(bob.get_text().unwrap(), "SBbbb");

        // Each order merges the two PRISTINE replicas: commutativity, not idempotence.
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

    /// The MERGE order is what is permuted; `load` sorts by id, so insert order proves nothing.
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

    #[test]
    fn positions_are_char_indices_not_byte_offsets() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str(0, "héllo wörld").unwrap();
        doc.insert(5, '!').unwrap();
        assert_eq!(doc.get_text().unwrap(), "héllo! wörld");
    }

    #[test]
    #[should_panic(expected = "migration")]
    fn insert_panics_during_migration() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        env::with_merge_mode(|| {
            let _ = doc.insert(0, 'H');
        });
    }

    #[test]
    #[should_panic(expected = "migration")]
    fn insert_str_panics_during_migration() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        env::with_merge_mode(|| {
            let _ = doc.insert_str(0, "Hi");
        });
    }

    #[test]
    #[should_panic(expected = "migration")]
    fn apply_delta_panics_during_migration() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        env::with_merge_mode(|| {
            let _ignored = doc.apply_delta(&[TextOp::Insert("H".to_owned())]);
        });
    }

    #[test]
    #[should_panic(expected = "migration")]
    fn insert_str_at_panics_during_migration() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        env::with_merge_mode(|| {
            let _ignored = doc.insert_str_at(&Anchor::Start, "H");
        });
    }

    #[test]
    fn apply_delta__replaces_a_word_in_one_call() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str(0, "hello world").unwrap();
        doc.apply_delta(&[
            TextOp::Retain(6),
            TextOp::Delete(5),
            TextOp::Insert("there".to_owned()),
        ])
        .unwrap();
        assert_eq!(doc.get_text().unwrap(), "hello there");
    }

    #[test]
    fn apply_delta__a_failing_op_stores_nothing() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        doc.insert_str(0, "abc").unwrap();
        let before = stored(&doc);

        let result = doc.apply_delta(&[
            TextOp::Delete(1),
            TextOp::Insert("X".to_owned()),
            TextOp::Retain(9),
            TextOp::Insert("Y".to_owned()),
        ]);
        assert!(result.is_err());
        assert_eq!(stored(&doc), before);
    }

    #[test]
    fn insert_str_with_replica_is_allowed_during_migration() {
        env::reset_for_testing();
        let mut doc = Root::new(FugueText::new);
        env::with_merge_mode(|| {
            doc.insert_str_with_replica(0, 7, "H").unwrap();
        });
        assert_eq!(doc.len().unwrap(), 1);
    }

    /// The premise for gating the `FugueTextBlock` join arm to `WriteOrigin::Applied`.
    #[test]
    fn local_writes__are_lattice_supersets_of_what_they_overwrite() {
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

        let before = stored(&doc);
        doc.insert(5, '!').unwrap();
        assert_superset(&before, &stored(&doc), "append coalescing");

        let before = stored(&doc);
        doc.insert(2, 'X').unwrap();
        assert_superset(&before, &stored(&doc), "mid-run insert");

        let before = stored(&doc);
        doc.delete(1).unwrap();
        assert_superset(&before, &stored(&doc), "delete");

        let before = stored(&doc);
        doc.delete_range(0, 3).unwrap();
        assert_superset(&before, &stored(&doc), "delete_range");
    }

    /// Bits may be set past this copy's text, as a short copy of a run that grew elsewhere has.
    fn random_block(rng: &mut StdRng, start_id: BlockId) -> TextBlock {
        let text: String = (0..rng.random_range(..JOIN_LAW_MAX_NODES + 1))
            .map(|_| JOIN_LAW_POOL[rng.random_range(..JOIN_LAW_POOL.len())])
            .collect();
        let mut tombstones = Vec::new();
        for offset in 0..JOIN_LAW_MAX_NODES {
            if rng.random_range(..4_usize) == 0 {
                tomb_set(&mut tombstones, offset);
            }
        }
        TextBlock {
            start_id,
            text,
            parent: (rng.random_range(..2_usize) == 0).then_some(start_id),
            side: if rng.random_range(..2_usize) == 0 {
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

    /// Asserted on the borsh bytes: the Merkle hash is taken over exactly those.
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

    /// Fixtures are ARBITRARY copies of one key, not the prefix pairs ordinary editing produces.
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

        let mut seeds = StdRng::seed_from_u64(JOIN_LAW_SEED);
        for _ in 0..JOIN_LAW_ROUNDS {
            let seed = seeds.random::<u64>();
            let mut rng = StdRng::seed_from_u64(seed);
            check_join_laws(
                &format!("seed {seed:#x}"),
                &random_block(&mut rng, id),
                &random_block(&mut rng, id),
                &random_block(&mut rng, id),
            );
        }
    }

    fn cap_text(count: usize) -> String {
        (0..count).map(|i| CAP_POOL[i % CAP_POOL.len()]).collect()
    }

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

    /// The trailing append proves `next_counter` survived the chunking.
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

    /// The model has no runs, so it is an oracle for the whole block layer.
    #[test]
    fn random_scripts_agree_with_the_model() {
        env::reset_for_testing();
        let mut blocked = doc_in::<MockedStorage<878>>("differential");
        let mut model = Model::default();

        let mut rng = StdRng::seed_from_u64(DIFFERENTIAL_SEED);
        let mut length = 0_usize;
        let mut minted = 0_usize;
        for round in 0..DIFFERENTIAL_ROUNDS {
            let replica = rng.random_range(..3_usize) as u64 + 1;
            let pos = rng.random_range(..length + 1);
            let budget = minted < DIFFERENTIAL_MAX_NODES;
            let text = match rng.random_range(..10_usize) {
                // A paste whose length straddles the cap.
                0..=2 if budget => cap_text(1 + rng.random_range(..MAX_RUN_LEN + MAX_RUN_LEN / 2)),
                3..=6 if budget => cap_text(1 + rng.random_range(..4_usize)),
                choice => {
                    let span = if choice == 7 {
                        1
                    } else {
                        1 + rng.random_range(..64_usize)
                    };
                    blocked.delete_range(pos, pos + span).unwrap();
                    model.delete_range(pos, pos + span);
                    String::new()
                }
            };
            if !text.is_empty() {
                blocked
                    .insert_str_with_replica(pos, replica, &text)
                    .unwrap();
                model.insert_str(pos, replica, &text);
                minted += text.chars().count();
            }
            let observed = blocked.get_text().unwrap();
            assert_eq!(
                observed,
                model.text(),
                "round {round} of seed {DIFFERENTIAL_SEED:#x} diverged from the model"
            );
            length = observed.chars().count();
        }
        assert!(
            minted > 2 * MAX_RUN_LEN,
            "the sweep never pasted past the cap: {minted} characters minted"
        );
    }
}

/// Convergence alone is not an oracle: a lossy merge converges on the same wrong text.
#[cfg(test)]
mod model_tests {
    use std::collections::BTreeMap;

    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    use super::{doc_in, FugueText, TextBlock};
    use crate::collections::fugue::{FugueNode, FugueTree};
    use crate::env;
    use crate::store::{MockedStorage, StorageAdaptor};

    /// The oracle: Algorithm 1 with the same id allocation `FugueText` uses.
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

        /// The CRDT join: every node, delete-wins per node, read through a fresh tree.
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

    /// `Ins` at the end coalesces; `Ins` in the middle mints a separate run.
    #[derive(Clone, Debug)]
    pub(super) enum Op {
        Ins(usize, &'static str),
        Del(usize, usize),
    }

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

    fn seed<S: StorageAdaptor>(doc: &mut FugueText<S>, model: &mut Model) {
        doc.insert_str_with_replica(0, 0, "seed").unwrap();
        model.insert_str(0, 0, "seed");
    }

    /// Edit concurrently, then merge in BOTH orders from the pristine states.
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

    /// One appending insert (coalesces), two mid-document inserts (do not), and two deletes.
    pub(super) const ALPHABET: [Op; 5] = [
        Op::Ins(usize::MAX, "x"),
        Op::Ins(1, "y"),
        Op::Ins(2, "zz"),
        Op::Del(1, 1),
        Op::Del(0, 3),
    ];

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

    #[test]
    fn fuzz__two_hundred_seeds_match_the_model() {
        env::reset_for_testing();
        const WORDS: [&str; 4] = ["a", "bc", "d", "ef"];

        let mut rng = StdRng::seed_from_u64(0x_f0_5e_ed_01);
        for seed in 0..200_usize {
            let script = |rng: &mut StdRng| -> Vec<Op> {
                let count = 1 + rng.random_range(..4_usize);
                (0..count)
                    .map(|_| {
                        if rng.random_range(..3_usize) == 0 {
                            Op::Del(rng.random_range(..8_usize), 1 + rng.random_range(..3_usize))
                        } else {
                            Op::Ins(
                                rng.random_range(..8_usize),
                                WORDS[rng.random_range(..WORDS.len())],
                            )
                        }
                    })
                    .collect()
            };
            let left = script(&mut rng);
            let right = script(&mut rng);
            scenario(&format!("fz{seed}"), &left, &right);
        }
    }

    /// Merge relies on it: `merged != mine` decides whether to write.
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

/// The batched resolver must answer exactly what the per-anchor walk answers,
/// because every rich-text read trusts it instead of [`FugueText::resolve`].
#[cfg(test)]
mod position_index_tests {
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    use super::{anchor_at_in, doc_in, Anchor, Bias, FugueText, PositionIndex};
    use crate::env;
    use crate::store::{MockedStorage, StorageAdaptor};

    const INDEX_SEED: u64 = 0x_1d_e4_5e_ed;
    const INDEX_DOCS: usize = 20; // documents built, each with tombstones and several writers

    fn random_doc<S: StorageAdaptor>(doc: &mut FugueText<S>, rng: &mut StdRng) {
        const WORDS: [&str; 4] = ["a", "bc", "déf", "日本"];
        for _ in 0..10 {
            let len = doc.len().unwrap();
            if len > 0 && rng.random_range(..3_usize) == 0 {
                let start = rng.random_range(..len);
                doc.delete_range(start, start + 1 + rng.random_range(..2_usize))
                    .unwrap();
            } else {
                let pos = rng.random_range(..len + 1);
                let replica = 1 + rng.random_range(..3_usize) as u64;
                doc.insert_str_with_replica(pos, replica, WORDS[rng.random_range(..WORDS.len())])
                    .unwrap();
            }
        }
    }

    #[test]
    fn resolve__index_and_tree_walk_agree_on_every_anchor() {
        env::reset_for_testing();
        type S = MockedStorage<884>;
        let mut rng = StdRng::seed_from_u64(INDEX_SEED);
        let mut checked = 0_usize;

        for seed in 0..INDEX_DOCS {
            let mut doc = doc_in::<S>(&format!("pidx{seed}"));
            random_doc(&mut doc, &mut rng);

            let tree = doc.tree().unwrap();
            let index = PositionIndex::build(&tree);
            let len = doc.len().unwrap();
            assert_eq!(index.resolve(&Anchor::End), Some(len));

            let mut anchors = vec![Anchor::Start, Anchor::End];
            for pos in 0..=len {
                for bias in [Bias::Before, Bias::After] {
                    anchors.push(anchor_at_in(&tree, pos, bias).unwrap());
                }
            }
            // Every node, tombstoned ones included: a mark outlives its characters.
            for node in tree.nodes() {
                for bias in [Bias::Before, Bias::After] {
                    anchors.push(Anchor::Char { id: node.id, bias });
                }
            }

            for anchor in &anchors {
                assert_eq!(
                    index.resolve(anchor),
                    Some(doc.resolve(anchor).unwrap()),
                    "seed {seed:#x}: index and walk disagree on {anchor:?}"
                );
                checked += 1;
            }
        }
        assert!(
            checked > 500,
            "the sweep resolved almost nothing: {checked}"
        );
    }

    #[test]
    fn resolve__an_unknown_character_is_none_where_the_walk_errors() {
        env::reset_for_testing();
        type S = MockedStorage<885>;
        let mut doc = doc_in::<S>("pidx-unknown");
        doc.insert_str_with_replica(0, 1, "hello").unwrap();

        let absent = Anchor::Char {
            id: (0x_dead_beef, 7),
            bias: Bias::Before,
        };
        let tree = doc.tree().unwrap();
        assert_eq!(PositionIndex::build(&tree).resolve(&absent), None);
        assert!(
            doc.resolve(&absent)
                .unwrap_err()
                .to_string()
                .contains("unknown character"),
            "the walk must still be the erroring resolver"
        );
    }
}

#[cfg(test)]
mod positional_read_tests {
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    use super::{build_tree, doc_in, find_block, BlockId, FugueText};
    use crate::collections::Root;
    use crate::env;
    use crate::store::{MockedStorage, StorageAdaptor};

    /// Appends (which coalesce), mid-document inserts (which do not) and deletes.
    fn random_doc<S: StorageAdaptor>(doc: &mut FugueText<S>, rng: &mut StdRng, ops: usize) {
        const WORDS: [&str; 5] = ["a", "bc", "déf", "ghij", "日本"];
        for _ in 0..ops {
            let len = doc.len().unwrap();
            if rng.random_range(..4_usize) == 0 && len > 0 {
                let start = rng.random_range(..len);
                doc.delete_range(start, start + 1 + rng.random_range(..3_usize))
                    .unwrap();
            } else {
                let pos = rng.random_range(..len + 1);
                let replica = 1 + rng.random_range(..3_usize) as u64;
                doc.insert_str_with_replica(pos, replica, WORDS[rng.random_range(..WORDS.len())])
                    .unwrap();
            }
        }
    }

    #[test]
    fn text_range__equals_the_get_text_slice_over_random_documents() {
        env::reset_for_testing();
        type S = MockedStorage<880>;
        let mut rng = StdRng::seed_from_u64(0x_1d_ec_0d_e5);

        for seed in 0..25_usize {
            let mut doc = doc_in::<S>(&format!("tr{seed}"));
            random_doc(&mut doc, &mut rng, 8);
            let text = doc.get_text().unwrap();
            let chars: Vec<char> = text.chars().collect();

            for _ in 0..12 {
                let a = rng.random_range(..chars.len() + 3);
                let b = a + rng.random_range(..chars.len() + 3);
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

    #[test]
    fn char_at__equals_chars_nth_including_out_of_bounds() {
        env::reset_for_testing();
        type S = MockedStorage<881>;
        let mut rng = StdRng::seed_from_u64(0x_c4_a2_a7_11);

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

    /// What lets [`find_block`] be a binary search.
    #[test]
    fn stored_blocks__cover_every_authoritative_node_exactly_once() {
        env::reset_for_testing();
        type S = MockedStorage<882>;
        let mut rng = StdRng::seed_from_u64(0x_ca_2d_11_0a);

        for seed in 0..15_usize {
            let mut doc = doc_in::<S>(&format!("card{seed}"));
            random_doc(&mut doc, &mut rng, 8);

            let loaded = doc.load().unwrap();
            let tree = build_tree(&loaded).unwrap();

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

    #[test]
    fn len__agrees_with_get_text() {
        env::reset_for_testing();
        type S = MockedStorage<887>;
        let mut rng = StdRng::seed_from_u64(0x_5e_11_00_02);

        for seed in 0..15_usize {
            let mut doc = doc_in::<S>(&format!("len{seed}"));
            random_doc(&mut doc, &mut rng, 8);
            let text = doc.get_text().unwrap();
            assert_eq!(doc.len().unwrap(), text.chars().count());
            assert_eq!(doc.is_empty().unwrap(), text.is_empty());
        }
    }
}

/// Convergence through the REAL receive path, [`Interface::apply_action`].
#[cfg(test)]
mod apply_path_tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    use super::model_tests::{Model, Op, ALPHABET};
    use super::{BlockId, BlockKey, FugueText, TextBlock, TextOp, MAX_RUN_LEN};
    use crate::collections::{CrdtType, Root};
    use crate::delta::{clear_pending_delta, StorageDelta};
    use crate::env::{self, RuntimeEnv};
    use crate::index::Index;
    use crate::interface::{ApplyContext, Interface};
    use crate::store::{Key, MainStorage};

    /// Kept local so these tests need no feature flag.
    type Store = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;

    /// Must be the native default: `ROOT_ID` is a process-global `LazyLock` seeded from the first.
    const CONTEXT_ID: [u8; 32] = [236_u8; 32];

    /// Both replicas must derive the SAME collection id.
    const FIELD: &str = "apply_path_doc";

    const CAP_SPAN: usize = MAX_RUN_LEN + 3; // both replicas cross the cap before syncing
    const PASTE_LEN: usize = 600; // past two run caps
    const PASTE_BYTES_PER_CHAR: usize = 8; // a per-character path blows straight past this
    const EQUIV_SEED: u64 = 0x_ba_7c_1e_d0;
    const DELTA_SEED: u64 = 0x_de_17_a5_ed;
    const DELTA_ROUNDS: usize = 30;
    const DELTA_MAX_OPS: usize = 6;
    const DELTA_MAX_DELETE: usize = 6;
    const DELTA_LENGTHS: [usize; 4] = [1, 2, 7, 300]; // the last crosses the run cap
    const EQUIV_ROUNDS: usize = 40;
    const EQUIV_MAX_NODES: usize = 1_400; // the per-character oracle is quadratic in this
    const EQUIV_POOL: [char; 4] = ['a', 'é', '日', 'Z']; // mixed widths: bytes != nodes
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

    /// The device id is what makes two replicas mint distinct nodes.
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

    /// Two replicas holding EQUAL-LENGTH, DIFFERENT text for one block key.
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

    /// One writer coalescing at the END of a run while another deletes inside
    /// it, both rewriting key `(A,0)`: an untagged LWW resolution erases a side,
    /// and both replicas still AGREE either way, so only the VALUE catches it.
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

    /// Run one two-replica scenario end to end through the APPLY path. The
    /// counterpart of `model_tests::scenario`: identical script and oracle, but
    /// the reconciliation path a node actually runs.
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
    /// where the coalesce-versus-delete collision lives.
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

    /// EXHAUSTIVE, not sampled: the same 5^4 = 625 two-op scripts `model_tests`
    /// runs through `merge_blocks_from`, reconciled through `apply_action`.
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
    /// Same seeds, generator and seed base as its `merge_blocks_from` twin in
    /// `model_tests`, so a failure here has a directly comparable counterpart.
    #[test]
    fn fuzz__two_hundred_seeds_match_the_model_through_the_apply_path() {
        const SEEDS: usize = 200;
        const WORDS: [&str; 4] = ["a", "bc", "d", "ef"];

        let mut rng = StdRng::seed_from_u64(0x_f0_5e_ed_01);
        for seed in 0..SEEDS {
            let script = |rng: &mut StdRng| -> Vec<Op> {
                let count = 1 + rng.random_range(..4_usize);
                (0..count)
                    .map(|_| {
                        if rng.random_range(..3_usize) == 0 {
                            Op::Del(rng.random_range(..8_usize), 1 + rng.random_range(..3_usize))
                        } else {
                            Op::Ins(
                                rng.random_range(..8_usize),
                                WORDS[rng.random_range(..WORDS.len())],
                            )
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
        Delta(Vec<TextOp>),
    }

    /// The oracle for `apply_delta`: the same ops, one stored call each.
    fn apply_ops_one_at_a_time(doc: &mut FugueText<MainStorage>, ops: &[TextOp]) {
        let mut pos = 0_usize;
        for op in ops {
            match *op {
                TextOp::Retain(count) => pos += count,
                TextOp::Insert(ref text) => {
                    doc.insert_str(pos, text).expect("insert should succeed");
                    pos += text.chars().count();
                }
                TextOp::Delete(count) => drop(
                    doc.delete_range(pos, pos + count)
                        .expect("delete should succeed"),
                ),
            }
        }
    }

    /// Random editor changes, a pure function of the seed; a `Delete` may overrun the end.
    fn delta_script() -> Vec<Step> {
        let mut rng = StdRng::seed_from_u64(DELTA_SEED);
        let mut len = 0_usize;
        (0..DELTA_ROUNDS)
            .map(|_| {
                let mut ops = Vec::new();
                let (mut cursor, mut next_len) = (0_usize, len);
                for _ in 0..1 + rng.random_range(..DELTA_MAX_OPS) {
                    let left = len - cursor;
                    match rng.random_range(..3_usize) {
                        0 if left > 0 => {
                            let count = rng.random_range(..left + 1);
                            cursor += count;
                            ops.push(TextOp::Retain(count));
                        }
                        1 if left > 0 => {
                            let count = 1 + rng.random_range(..DELTA_MAX_DELETE);
                            let removed = count.min(left);
                            cursor += removed;
                            next_len -= removed;
                            ops.push(TextOp::Delete(count));
                        }
                        _ => {
                            let count = DELTA_LENGTHS[rng.random_range(..DELTA_LENGTHS.len())];
                            next_len += count;
                            ops.push(TextOp::Insert(
                                (0..count)
                                    .map(|_| EQUIV_POOL[rng.random_range(..EQUIV_POOL.len())])
                                    .collect(),
                            ));
                        }
                    }
                }
                len = next_len;
                Step::Delta(ops)
            })
            .collect()
    }

    /// A random edit script, a pure function of the seed (the length is tracked
    /// arithmetically, not read back, so both paths replay the same one), over
    /// the four positions the Fugue insert rule resolves differently.
    fn equivalence_script() -> Vec<Step> {
        let mut rng = StdRng::seed_from_u64(EQUIV_SEED);
        let mut script = Vec::with_capacity(EQUIV_ROUNDS);
        let mut len = 0_usize;
        let mut minted = 0_usize;

        for round in 0..EQUIV_ROUNDS {
            if round > 0 && rng.random_range(..4_usize) == 0 {
                let start = rng.random_range(..len + 1);
                let span = 1 + rng.random_range(..8_usize);
                len -= (start + span).min(len).saturating_sub(start);
                script.push(Step::Delete { start, span });
                continue;
            }
            let drawn = EQUIV_LENGTHS[rng.random_range(..EQUIV_LENGTHS.len())];
            let count = if minted + drawn > EQUIV_MAX_NODES {
                rng.random_range(..4_usize)
            } else {
                drawn
            };
            let pos = match rng.random_range(..4_usize) {
                0 => 0,
                1 => len / 2,
                2 => len,
                _ => rng.random_range(..len + 1),
            };
            script.push(Step::Insert {
                pos,
                replica: 1 + rng.random_range(..3_usize) as u64,
                text: (0..count)
                    .map(|_| EQUIV_POOL[rng.random_range(..EQUIV_POOL.len())])
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
                            doc.insert_str_with_replica(pos, replica, text).map(drop)
                        }
                        .expect("insert should succeed");
                    }
                    Step::Delete { start, span } => {
                        doc.delete_range(start, start + span)
                            .expect("delete should succeed");
                    }
                    Step::Delta(ref ops) if per_char => apply_ops_one_at_a_time(&mut doc, ops),
                    Step::Delta(ref ops) => {
                        let _undo = doc.apply_delta(ops).expect("delta should succeed");
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

    /// Names the differing block instead of dumping the whole set.
    fn assert_same_stored(seed: u64, got: &StoredState, want: &StoredState) {
        let ids = |blocks: &[(BlockId, TextBlock)]| -> Vec<BlockId> {
            blocks.iter().map(|(id, _)| *id).collect()
        };
        assert!(!want.0.is_empty(), "seed {seed:#x} stored nothing");
        assert_eq!(
            ids(&got.0),
            ids(&want.0),
            "seed {seed:#x}: the batched path stored a different block set"
        );
        for ((id, block), (_, expected)) in got.0.iter().zip(&want.0) {
            assert_eq!(
                block, expected,
                "seed {seed:#x}: block {id:?} differs from the unbatched path's"
            );
        }
        assert_eq!(got.1, want.1, "seed {seed:#x}: CRDT tags differ");
        assert_eq!(got.2, want.2, "seed {seed:#x}: Merkle roots differ");
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
                Step::Delete { .. } | Step::Delta(_) => None,
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

        assert_same_stored(EQUIV_SEED, &replay(&script, false), &replay(&script, true));
    }

    /// An editor change must store exactly what its ops stored one call at a time.
    #[test]
    fn apply_delta_stores_what_the_ops_stored_one_at_a_time() {
        let script = delta_script();
        let ops = || {
            script.iter().flat_map(|step| match *step {
                Step::Delta(ref ops) => ops.as_slice(),
                _ => &[],
            })
        };
        for (what, drawn) in [
            ("a retain", ops().any(|op| matches!(*op, TextOp::Retain(_)))),
            ("a delete", ops().any(|op| matches!(*op, TextOp::Delete(_)))),
            (
                "an insert past the cap",
                ops().any(
                    |op| matches!(*op, TextOp::Insert(ref s) if s.chars().count() > MAX_RUN_LEN),
                ),
            ),
            (
                "a change of several ops",
                script
                    .iter()
                    .any(|step| matches!(*step, Step::Delta(ref ops) if ops.len() > 3)),
            ),
        ] {
            assert!(drawn, "seed {DELTA_SEED:#x} never drew {what}");
        }

        assert_same_stored(DELTA_SEED, &replay(&script, false), &replay(&script, true));
    }

    /// One action per touched block, however many ops touch it.
    #[test]
    fn one_apply_delta_emits_one_action_per_block_it_touches() {
        let store = new_store();
        let dev = device(1);
        clear_pending_delta();
        env::with_runtime_env(env_for(&store, dev), || {
            Root::new(|| FugueText::<MainStorage>::new_with_field_name(FIELD)).commit();
            let _ignored = env::take_last_artifact();
        });
        let _seeded = edit(&store, dev, |doc| {
            doc.insert_str(0, "hello world").expect("seed text");
        });

        // Two deletes and a coalescing append all land in the one seeded block.
        let delta = edit(&store, dev, |doc| {
            doc.apply_delta(&[
                TextOp::Delete(1),
                TextOp::Retain(4),
                TextOp::Delete(1),
                TextOp::Retain(5),
                TextOp::Insert("!".to_owned()),
            ])
            .expect("delta should succeed");
            assert_eq!(doc.get_text().expect("text"), "elloworld!");
        });
        let actions = match borsh::from_slice::<StorageDelta>(&delta).expect("delta should decode")
        {
            StorageDelta::Actions(actions) => actions,
            StorageDelta::CausalActions { actions, .. } => actions,
        };
        let written = actions.iter().filter(|a| !a.id().is_root()).count();
        assert_eq!(written, 2, "one block plus the collection's own element");
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

/// Documents outlive code, so the stored layout is frozen here, not merely described.
#[cfg(test)]
mod golden_tests {
    use super::{join_block, BlockId, BlockKey, BlockSide, TextBlock, MAX_RUN_LEN};

    /// Multi-byte text, a parent, side L, and a trimmed bitmap with a gap byte.
    fn golden_block() -> TextBlock {
        TextBlock {
            start_id: BlockId::new(0x0102_0304_0506_0708, 0x090A_0B0C),
            text: "a\u{e9}\u{65e5}\u{1F600}".to_owned(),
            parent: Some(BlockId::new(9, 7)),
            side: BlockSide::L,
            tombstones: vec![0b0000_1001, 0b0100_0000],
        }
    }

    #[test]
    fn text_block__borsh_layout_is_frozen() {
        let block = golden_block();
        let bytes = borsh::to_vec(&block).unwrap();
        assert_eq!(
            bytes,
            [
                8, 7, 6, 5, 4, 3, 2, 1, // start_id.replica, u64 little-endian
                12, 11, 10, 9, // start_id.counter, u32 little-endian
                10, 0, 0, 0, // text, byte length then UTF-8
                97, 195, 169, 230, 151, 165, 240, 159, 152, 128, //
                1, 9, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, // parent: Some(BlockId)
                0, // side: L
                2, 0, 0, 0, 9, 64, // tombstones, byte length then bits
            ]
        );
        assert_eq!(borsh::from_slice::<TextBlock>(&bytes).unwrap(), block);
    }

    #[test]
    fn text_block__side_r_and_a_rootless_parent_are_frozen() {
        let block = TextBlock {
            parent: None,
            side: BlockSide::R,
            ..golden_block()
        };
        assert_eq!(
            borsh::to_vec(&block).unwrap(),
            [
                8, 7, 6, 5, 4, 3, 2, 1, 12, 11, 10, 9, 10, 0, 0, 0, 97, 195, 169, 230, 151, 165,
                240, 159, 152, 128, //
                0,   // parent: None
                1,   // side: R
                2, 0, 0, 0, 9, 64,
            ]
        );
    }

    /// The map key is the `BlockId` alone: the value duplicates it, the key does not.
    #[test]
    fn block_key__borsh_layout_is_frozen() {
        let key = BlockKey::new(golden_block().start_id);
        assert_eq!(
            borsh::to_vec(&key).unwrap(),
            [8, 7, 6, 5, 4, 3, 2, 1, 12, 11, 10, 9]
        );
        assert_eq!(key.as_ref(), borsh::to_vec(&key).unwrap());
    }

    /// Every stored document partitions its counter space at this cap, and a full
    /// run is never rewritten, so changing it re-chunks every document ever written.
    #[test]
    fn max_run_len__is_frozen() {
        assert_eq!(
            MAX_RUN_LEN, 256,
            "stored documents are chunked at this cap and full runs are never rewritten, \
             so an existing document cannot be read back under another value"
        );
    }

    /// One pair per tie-break level of `(node count, text, parent, side)`.
    #[test]
    fn join_block__ranks_by_each_tie_break_level_in_turn() {
        let base = |text: &str, parent: Option<BlockId>, side: BlockSide| TextBlock {
            start_id: BlockId::new(1, 0),
            text: text.to_owned(),
            parent,
            side,
            tombstones: Vec::new(),
        };
        let (l, r) = (BlockSide::L, BlockSide::R);
        let (low, high) = (Some(BlockId::new(1, 1)), Some(BlockId::new(1, 2)));

        let table = [
            (
                "node count beats text",
                base("zz", None, r),
                base("aaa", None, r),
            ),
            (
                "equal count: greater text wins",
                base("ab", None, r),
                base("ba", None, r),
            ),
            (
                "equal count and text: greater parent wins",
                base("ab", low, r),
                base("ab", high, r),
            ),
            (
                "a rootless block loses to a parented one",
                base("ab", None, r),
                base("ab", low, r),
            ),
            (
                "equal count, text and parent: R beats L",
                base("ab", low, l),
                base("ab", low, r),
            ),
        ];

        for (what, loser, winner) in table {
            let mut keeps = winner.clone();
            join_block(&mut keeps, loser.clone());
            assert_eq!(keeps, winner, "{what}: the winner did not keep its record");

            let mut takes = loser;
            join_block(&mut takes, winner.clone());
            assert_eq!(takes, winner, "{what}: the loser did not take the winner");
        }
    }

    /// The bitmap is a union whichever side of the rank wins.
    #[test]
    fn join_block__unions_tombstones_across_the_rank() {
        let block = |text: &str, tombstones: Vec<u8>| TextBlock {
            start_id: BlockId::new(1, 0),
            text: text.to_owned(),
            parent: None,
            side: BlockSide::R,
            tombstones,
        };
        let mut winner = block("abc", vec![0b0000_0001]);
        join_block(&mut winner, block("a", vec![0b0000_0100]));
        assert_eq!(winner.text, "abc");
        assert_eq!(winner.tombstones, vec![0b0000_0101]);

        let mut loser = block("a", vec![0b0000_0100]);
        join_block(&mut loser, block("abc", vec![0b0000_0001]));
        assert_eq!(loser.text, "abc");
        assert_eq!(loser.tombstones, vec![0b0000_0101]);
    }
}

/// Positions are Unicode scalar values on every path, and a run never splits one.
#[cfg(test)]
mod scalar_value_tests {
    use super::{doc_in, BlockId, FugueText, TextBlock, TextOp, MAX_RUN_LEN};
    use crate::collections::fugue_text::{Anchor, Bias};
    use crate::env;
    use crate::store::{MockedStorage, StorageAdaptor};

    const GRINNING: &str = "\u{1F600}"; // astral: 1 scalar value, 2 UTF-16 units, 4 bytes
    const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"; // 5 scalar values
    const ACUTE: &str = "e\u{301}"; // a combining mark is its own scalar value

    fn utf16_len(text: &str) -> usize {
        text.chars().map(char::len_utf16).sum()
    }

    #[test]
    fn every_position_counts_scalar_values() {
        env::reset_for_testing();
        let mut doc = doc_in::<MockedStorage<890>>("scalar-units");
        let seed = format!("{GRINNING}{FAMILY}{ACUTE}");
        doc.insert_str_with_replica(0, 7, &seed).unwrap();

        assert_eq!(doc.len().unwrap(), 8);
        assert_eq!(utf16_len(&seed), 12, "a browser would count 12");
        assert_eq!(seed.len(), 25, "the bytes are a third unit again");

        assert_eq!(doc.char_at(0).unwrap(), Some('\u{1F600}'));
        assert_eq!(doc.char_at(2).unwrap(), Some('\u{200D}'));
        assert_eq!(doc.char_at(7).unwrap(), Some('\u{301}'));
        assert_eq!(doc.char_at(8).unwrap(), None);
        assert_eq!(doc.text_range(1, 6).unwrap(), FAMILY);

        // Position 3 is inside the family sequence, between two of its parts.
        doc.insert_str_with_replica(3, 7, GRINNING).unwrap();
        assert_eq!(doc.len().unwrap(), 9);
        assert_eq!(doc.char_at(3).unwrap(), Some('\u{1F600}'));

        // Deleting one position takes one scalar value, not one UTF-16 unit.
        let removed = doc.delete_range(3, 4).unwrap().unwrap();
        assert_eq!(removed.text, GRINNING);
        assert_eq!(doc.get_text().unwrap(), seed);
    }

    #[test]
    fn anchors_address_scalar_values() {
        env::reset_for_testing();
        let mut doc = doc_in::<MockedStorage<891>>("scalar-anchors");
        doc.insert_str_with_replica(0, 7, &format!("{GRINNING}{GRINNING}{ACUTE}"))
            .unwrap();

        let after_first = doc.anchor_at(1, Bias::After).unwrap();
        let before_mark = doc.anchor_at(3, Bias::Before).unwrap();
        assert!(matches!(after_first, Anchor::Char { .. }));

        doc.insert_str_with_replica(0, 7, FAMILY).unwrap();
        assert_eq!(doc.resolve(&after_first).unwrap(), 6);
        assert_eq!(doc.resolve(&before_mark).unwrap(), 8);
        assert_eq!(
            doc.resolve_many(&[after_first, before_mark]).unwrap(),
            [6, 8]
        );
    }

    #[test]
    fn apply_delta_counts_scalar_values() {
        env::reset_for_testing();
        let mut doc = doc_in::<MockedStorage<892>>("scalar-delta");
        doc.insert_str_with_replica(0, 7, &format!("{GRINNING}{FAMILY}"))
            .unwrap();

        // Every count is scalar values, including the cursor walk between the ops.
        let steps = doc
            .apply_delta(&[
                TextOp::Retain(1),
                TextOp::Insert(ACUTE.to_owned()),
                TextOp::Retain(2),
                TextOp::Insert(GRINNING.to_owned()),
                TextOp::Delete(1),
            ])
            .unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(
            doc.get_text().unwrap(),
            "\u{1F600}e\u{301}\u{1F468}\u{200D}\u{1F600}\u{200D}\u{1F467}"
        );
        assert_eq!(doc.len().unwrap(), 8);
    }

    /// A run is capped in NODES, and one node holds one scalar value, so the
    /// boundary can never land inside a character's bytes.
    #[test]
    fn the_run_cap_never_splits_an_astral_character() {
        env::reset_for_testing();
        type S = MockedStorage<893>;
        let mut doc = doc_in::<S>("scalar-cap");
        let paste: String = GRINNING.repeat(MAX_RUN_LEN + 3);
        doc.insert_str_with_replica(0, 7, &paste).unwrap();

        let blocks = stored(&doc);
        assert_eq!(blocks.len(), 2, "the paste must cross the cap");
        assert_eq!(blocks[0].1.text.chars().count(), MAX_RUN_LEN);
        assert_eq!(blocks[1].1.text.chars().count(), 3);
        for (id, block) in &blocks {
            assert!(
                block.text.chars().all(|c| c == '\u{1F600}'),
                "block {id:?} holds a split character"
            );
        }
        assert_eq!(doc.get_text().unwrap(), paste);

        // The stored rows survive a borsh round trip unchanged.
        let bytes = borsh::to_vec(&blocks).unwrap();
        assert_eq!(
            borsh::from_slice::<Vec<(BlockId, TextBlock)>>(&bytes).unwrap(),
            blocks
        );
    }

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
}
