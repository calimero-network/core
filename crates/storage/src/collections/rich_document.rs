//! `RichDocument` - an ordered list of blocks, each a [`RichText`] body plus
//! typed structure.
//!
//! Order comes from a spine [`FugueText`] of placeholder characters: creating a
//! block mints one, and the block's id IS that character's id, so it is stable
//! across every later move. A move mints a NEW slot and last-write-wins on the
//! block's `place`, which is why two concurrent moves settle on one placement
//! and can never duplicate a block.
//!
//! A block's mutable structure lives one row per field, not one row per block:
//! a map entry carries no `CrdtType`, so a whole-row reconciliation would drop a
//! concurrent write to a different field, and could resurrect a deleted block.
//! Deleting is a tombstone row, never `UnorderedMap::remove`, so an undelete is
//! an ordinary last write rather than a race against an index tombstone.
//!
//! The read is one rule: a block renders at the position its `place` resolves
//! to, ordered by id at a tie, and a block whose spine slot has not arrived
//! renders at the END rather than disappearing.

use std::collections::BTreeMap;
use std::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use super::crdt_meta::{MergeError, Mergeable};
use super::error::StoreError;
use super::fugue::RawId;
use super::fugue_text::{Anchor, Bias, FugueText, PositionIndex};
use super::mark_schema::{expand_for, MarkSchema};
use super::rich_text::{Attrs, DeltaOp, DeltaUndo, MarkId, RichText, Span};
use super::{CrdtType, LwwRegister, UnorderedMap, ValueRef};
use crate::store::{MainStorage, StorageAdaptor};

const BLOCKS_FIELD: &str = "__doc_blocks"; // child id namespace for the block map
const PROPS_FIELD: &str = "__block_props"; // child id namespace for a block's structure rows
const ATTRS_FIELD: &str = "__block_attrs"; // child id namespace for a block's attributes
const SPINE_SLOT: &str = "\u{FFFC}"; // one OBJECT REPLACEMENT CHARACTER per block ever created
const KIND: &str = "kind"; // property row key
const DEPTH: &str = "depth"; // property row key
const PLACE: &str = "place"; // property row key
const DELETED: &str = "deleted"; // property row key

/// A block's identity: the id of the spine character minted when it was
/// created. Unique, deterministic, and stable across moves.
///
/// JSON: the two-element array `[replica, counter]`, like every other id here.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct BlockId(pub RawId);

/// Storage key for a block; owns its serialized bytes for `AsRef<[u8]>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockKey {
    id: BlockId,
    bytes: Vec<u8>,
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

impl BorshSerialize for BlockKey {
    fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
        BorshSerialize::serialize(&self.id, writer)
    }
}

impl BorshDeserialize for BlockKey {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        Ok(Self::new(BlockId::deserialize_reader(reader)?))
    }
}

impl AsRef<[u8]> for BlockKey {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// One field of a block's mutable structure. The variant carries the field, so
/// a read never has to trust the row key and a write cannot name the wrong one.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub(crate) enum Prop {
    Kind(String),
    Depth(u8),
    Place(Anchor),
    Deleted(bool),
}

impl Prop {
    const fn key(&self) -> &'static str {
        match *self {
            Self::Kind(_) => KIND,
            Self::Depth(_) => DEPTH,
            Self::Place(_) => PLACE,
            Self::Deleted(_) => DELETED,
        }
    }
}

/// A block's structure, read in one pass over its property rows.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Structure {
    kind: String,
    depth: u8,
    place: Anchor,
    deleted: bool,
}

/// One block: a rich-text body, its attributes, and its structure rows.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct Block<Sc: MarkSchema, S: StorageAdaptor = MainStorage> {
    /// Duplicated from the map key so a row read in isolation is self-describing.
    id: BlockId,
    #[borsh(bound(serialize = "", deserialize = ""))]
    props: UnorderedMap<String, LwwRegister<Prop>, S>,
    #[borsh(bound(serialize = "", deserialize = ""))]
    attrs: UnorderedMap<String, LwwRegister<Option<String>>, S>,
    #[borsh(bound(serialize = "", deserialize = ""))]
    body: RichText<Sc, S>,
}

/// Re-key every child collection under the block's own entry id, so two
/// replicas that independently receive one block derive identical row ids.
impl<Sc: MarkSchema, S: StorageAdaptor> super::rekey::RekeyTarget for Block<Sc, S> {
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        self.props
            .reassign_deterministic_id_under(parent_id, PROPS_FIELD, CrdtType::UnorderedMap);
        self.attrs
            .reassign_deterministic_id_under(parent_id, ATTRS_FIELD, CrdtType::UnorderedMap);
        self.body.rekey_relative_to(parent_id);
    }
}

impl<Sc: MarkSchema, S: StorageAdaptor> Block<Sc, S> {
    fn new(id: BlockId) -> Self {
        let block = Self {
            id,
            props: UnorderedMap::new_internal(),
            attrs: UnorderedMap::new_internal(),
            body: RichText::new_internal(),
        };
        let _ignored = super::rekey::register_rekey::<Self>();
        block
    }

    /// Every structure field in one pass; an absent row takes its default,
    /// which is why creating a block writes no `place` and no `deleted`.
    fn structure(&self) -> Result<Structure, StoreError> {
        let mut out = Structure {
            kind: String::new(),
            depth: 0,
            place: Anchor::Char {
                id: self.id.0,
                bias: Bias::Before,
            },
            deleted: false,
        };
        for (_, register) in self.props.entries()? {
            match register.into_inner() {
                Prop::Kind(kind) => out.kind = kind,
                Prop::Depth(depth) => out.depth = depth,
                Prop::Place(place) => out.place = place,
                Prop::Deleted(deleted) => out.deleted = deleted,
            }
        }
        Ok(out)
    }

    fn set(&mut self, prop: Prop) -> Result<(), StoreError> {
        let _ignored = self
            .props
            .insert(prop.key().to_owned(), LwwRegister::new(prop))?;
        Ok(())
    }

    /// The attributes that are set, newest value per key; a `None` row means the
    /// key was removed and is filtered out rather than rendered as empty.
    fn attributes(&self) -> Result<BTreeMap<String, String>, StoreError> {
        Ok(self
            .attrs
            .entries()?
            .filter_map(|(key, register)| register.into_inner().map(|value| (key, value)))
            .collect())
    }
}

/// An ordered list of blocks, each with its own text, formatting and structure.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct RichDocument<Sc: MarkSchema, S: StorageAdaptor = MainStorage> {
    /// One placeholder character per block ever created: block order and stable
    /// block ids for free, with Fugue's non-interleaving guarantee.
    #[borsh(bound(serialize = "", deserialize = ""))]
    spine: FugueText<S>,
    #[borsh(bound(serialize = "", deserialize = ""))]
    blocks: UnorderedMap<BlockKey, Block<Sc, S>, S>,
    /// `fn() -> Sc` rather than `Sc`, so the schema decides nothing about this
    /// type's auto traits or variance. Never reaches a stored byte.
    #[borsh(skip)]
    schema: PhantomData<fn() -> Sc>,
}

/// Re-key both child collections under the storage parent so a nested document
/// converges, and register `Block` so its own children are re-keyed on insert.
impl<Sc: MarkSchema, S: StorageAdaptor> super::rekey::RekeyTarget for RichDocument<Sc, S> {
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        self.spine.rekey_relative_to(parent_id);
        self.blocks.reassign_deterministic_id_under(
            parent_id,
            BLOCKS_FIELD,
            CrdtType::UnorderedMap,
        );
    }

    fn register_nested_value_types() {
        super::rekey::register_rekey_cascade::<Block<Sc, MainStorage>>();
    }
}

impl<Sc: MarkSchema> RichDocument<Sc, MainStorage> {
    /// Create a new empty document with a random id.
    #[must_use]
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Create a new document with a deterministic id derived from `field_name`.
    #[must_use]
    pub fn new_with_field_name(field_name: &str) -> Self {
        let doc = Self {
            spine: FugueText::new_with_field_name(field_name),
            blocks: UnorderedMap::new_with_field_name_and_crdt_type(
                None,
                &format!("{field_name}{BLOCKS_FIELD}"),
                CrdtType::UnorderedMap,
            ),
            schema: PhantomData,
        };
        doc.register();
        doc
    }
}

impl<Sc: MarkSchema> Default for RichDocument<Sc, MainStorage> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Sc: MarkSchema, S: StorageAdaptor> RichDocument<Sc, S> {
    fn new_internal() -> Self {
        let doc = Self {
            spine: FugueText::new_internal(),
            blocks: UnorderedMap::new_internal(),
            schema: PhantomData,
        };
        doc.register();
        doc
    }

    /// Both thunks, in the constructor: a block row is an `UnorderedMap` value,
    /// so its own children only re-key if `Block` is in the registry first.
    fn register(&self) {
        let _ignored = super::rekey::register_rekey::<Self>();
        super::rekey::register_rekey_cascade::<Block<Sc, MainStorage>>();
    }

    /// Called by the `#[app::state]` macro after `init()`.
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        self.spine.reassign_deterministic_id(field_name);
        self.blocks
            .reassign_deterministic_id(&format!("{field_name}{BLOCKS_FIELD}"));
        self.blocks.set_collection_crdt_type(CrdtType::UnorderedMap);
    }

    // ---- structure writes ----

    /// Create a block directly after `after`, or at the top of the document when
    /// `after` is `None`. Panics inside a state migration, because it mints a
    /// spine id from the node-local device id.
    pub fn insert_block(
        &mut self,
        after: Option<BlockId>,
        kind: &str,
        depth: u8,
    ) -> Result<BlockId, StoreError> {
        let at = self.slot_for(after)?;
        let minted = self
            .spine
            .insert_str(at, SPINE_SLOT)?
            .ok_or_else(|| invalid("the spine minted no slot for a block"))?;

        let id = BlockId(minted.start);
        let key = BlockKey::new(id);
        let mut block = Block::new(id);
        // Re-key first, so the property rows below land under the entry id the
        // map is about to store the block at rather than under a random one.
        super::rekey::RekeyTarget::rekey_relative_to(&mut block, self.blocks.entry_id(&key));
        block.set(Prop::Kind(kind.to_owned()))?;
        block.set(Prop::Depth(depth))?;
        let _ignored = self.blocks.insert(key, block)?;
        Ok(id)
    }

    /// Tombstone `block`, returning whether it was already deleted.
    pub fn delete_block(&mut self, block: BlockId) -> Result<bool, StoreError> {
        let mut row = self.row(block)?;
        let was = row.structure()?.deleted;
        row.set(Prop::Deleted(true))?;
        Ok(was)
    }

    /// Move `block` directly after `after`, or to the top when `after` is `None`.
    ///
    /// Mints a NEW spine slot and last-write-wins on `place`, so the block id is
    /// unchanged and two concurrent moves settle on one placement. The old slot
    /// is left live and unreferenced: the read enumerates blocks, never spine
    /// characters, so a slot no block names is invisible.
    pub fn move_block(&mut self, block: BlockId, after: Option<BlockId>) -> Result<(), StoreError> {
        let mut row = self.row(block)?;
        let at = self.slot_for(after)?;
        let minted = self
            .spine
            .insert_str(at, SPINE_SLOT)?
            .ok_or_else(|| invalid("the spine minted no slot for a block"))?;
        row.set(Prop::Place(Anchor::Char {
            id: minted.start,
            bias: Bias::Before,
        }))
    }

    pub fn set_kind(&mut self, block: BlockId, kind: &str) -> Result<(), StoreError> {
        self.row(block)?.set(Prop::Kind(kind.to_owned()))
    }

    pub fn set_depth(&mut self, block: BlockId, depth: u8) -> Result<(), StoreError> {
        self.row(block)?.set(Prop::Depth(depth))
    }

    /// Set a block attribute, or remove it with `None`. Removal is its own row
    /// state, because an empty string is a value a caller may want to store.
    pub fn set_attr(
        &mut self,
        block: BlockId,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), StoreError> {
        let _ignored = self
            .row(block)?
            .attrs
            .insert(key.to_owned(), LwwRegister::new(value.map(str::to_owned)))?;
        Ok(())
    }

    /// Split `block` at visible position `at`, moving the tail's text AND its
    /// formatting into a new block placed immediately after.
    ///
    /// Marks anchored to the old characters cannot follow them, so the tail's
    /// resolved attributes are re-asserted as new mark rows over the new
    /// characters. A peer typing in the tail while this runs keeps its text in
    /// the FIRST block: this deletes only the characters that existed here.
    pub fn split_block(&mut self, block: BlockId, at: usize) -> Result<BlockId, StoreError> {
        let mut row = self.row(block)?;
        let len = row.body.len()?;
        if at > len {
            return Err(out_of_bounds(at));
        }
        let tail = slice_spans(&row.body.to_delta()?, at, len);
        declared_keys::<Sc>(&tail)?;
        let structure = row.structure()?;

        let new = self.insert_block(Some(block), &structure.kind, structure.depth)?;
        for (key, value) in row.attributes()? {
            self.set_attr(new, &key, Some(&value))?;
        }
        if at < len {
            let _undo = self.row(new)?.body.apply_delta(&carry(&tail))?;
            let _undo = row
                .body
                .apply_delta(&[retain(at), DeltaOp::Delete { delete: len - at }])?;
        }
        Ok(new)
    }

    /// Append `second`'s body, text and formatting, to `first` and tombstone
    /// `second`. The tombstoned body's rows survive, so the two halves are still
    /// addressable afterwards.
    pub fn merge_blocks(&mut self, first: BlockId, second: BlockId) -> Result<(), StoreError> {
        if first == second {
            return Err(invalid("a block cannot merge into itself"));
        }
        let mut head = self.row(first)?;
        let mut tail_row = self.row(second)?;
        let tail = tail_row.body.to_delta()?;
        declared_keys::<Sc>(&tail)?;

        if !tail.is_empty() {
            let mut ops = vec![retain(head.body.len()?)];
            ops.extend(carry(&tail));
            let _undo = head.body.apply_delta(&ops)?;
        }
        tail_row.set(Prop::Deleted(true))
    }

    // ---- body writes ----

    /// Apply one editor transaction to a block's body.
    pub fn apply_delta(
        &mut self,
        block: BlockId,
        ops: &[DeltaOp],
    ) -> Result<DeltaUndo, StoreError> {
        self.row(block)?.body.apply_delta(ops)
    }

    /// Set `key` over visible positions `start..end` of a block's body.
    pub fn mark(
        &mut self,
        block: BlockId,
        start: usize,
        end: usize,
        key: &str,
        value: Option<&str>,
    ) -> Result<Option<MarkId>, StoreError> {
        self.row(block)?.body.mark(start, end, key, value)
    }

    // ---- reads ----

    /// The ordered, non-deleted blocks.
    pub fn blocks(&self) -> Result<Vec<BlockView>, StoreError> {
        let index = PositionIndex::build(&self.spine.tree()?);
        let mut live: Vec<Placed<Sc, S>> = Vec::new();
        for (key, row) in self.blocks.entries()? {
            let structure = row.structure()?;
            if structure.deleted {
                continue;
            }
            let at = index.resolve(&structure.place);
            live.push(((at.is_none(), at, key.id()), row, structure));
        }
        // A block whose spine slot has not arrived sorts last, by id: content
        // that exists must never be invisible, and it self-heals on arrival.
        live.sort_by_key(|(key, _, _)| *key);

        live.into_iter()
            .map(|((_, _, id), row, structure)| view_of(id, &row, structure))
            .collect()
    }

    /// One block, or `None` when it is unknown or deleted.
    pub fn block(&self, block: BlockId) -> Result<Option<BlockView>, StoreError> {
        let Some(row) = self.blocks.get(&BlockKey::new(block))? else {
            return Ok(None);
        };
        let row = row.into_inner();
        let structure = row.structure()?;
        if structure.deleted {
            return Ok(None);
        }
        view_of(block, &row, structure).map(Some)
    }

    /// One block's rendered spans, with no spine rebuild: the read a binding
    /// does on every keystroke.
    pub fn block_delta(&self, block: BlockId) -> Result<Vec<Span>, StoreError> {
        self.row(block)?.body.to_delta()
    }

    // ---- internals ----

    fn row(&self, block: BlockId) -> Result<Block<Sc, S>, StoreError> {
        self.blocks
            .get(&BlockKey::new(block))?
            .map(ValueRef::into_inner)
            .ok_or_else(|| unknown_block(block))
    }

    /// The spine position a slot placed after `after` takes. A block whose own
    /// slot has not arrived renders at the end, so a block placed after it does
    /// too, which keeps the two reads agreeing.
    fn slot_for(&self, after: Option<BlockId>) -> Result<usize, StoreError> {
        let Some(after) = after else { return Ok(0) };
        let place = self.row(after)?.structure()?.place;
        let index = PositionIndex::build(&self.spine.tree()?);
        Ok(match index.resolve(&place) {
            Some(at) => at + 1,
            None => self.spine.len()?,
        })
    }

    /// Copy in `other`'s spine and block rows. A key both sides hold is merged
    /// field by field, because a block, unlike a mark, is mutable.
    pub(crate) fn merge_from(&mut self, other: &Self) -> Result<(), MergeError> {
        self.spine.merge_blocks_from(&other.spine)?;
        for (key, incoming) in other.blocks.entries()? {
            if let Some(mine) = self.blocks.get(&key)? {
                let mut mine = mine.into_inner();
                mine.props.merge(&incoming.props)?;
                mine.attrs.merge(&incoming.attrs)?;
                mine.body.merge_from(&incoming.body)?;
                continue;
            }
            // A storage tombstone means `self` removed it: do not resurrect.
            if crate::index::Index::<S>::is_deleted(self.blocks.entry_id(&key))
                .map_err(StoreError::from)?
            {
                continue;
            }
            let _ignored = self.blocks.insert(key, incoming)?;
        }
        Ok(())
    }
}

/// What a client receives for one block: its structure and its rendered spans.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockView {
    pub id: BlockId,
    pub kind: String,
    pub depth: u8,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, String>,
    pub spans: Vec<Span>,
}

/// A block ready to render: its sort key, its row, and the structure already
/// read off that row.
type Placed<Sc, S> = ((bool, Option<usize>, BlockId), Block<Sc, S>, Structure);

fn view_of<Sc: MarkSchema, S: StorageAdaptor>(
    id: BlockId,
    row: &Block<Sc, S>,
    structure: Structure,
) -> Result<BlockView, StoreError> {
    Ok(BlockView {
        id,
        kind: structure.kind,
        depth: structure.depth,
        attrs: row.attributes()?,
        spans: row.body.to_delta()?,
    })
}

/// The part of `spans` covering visible positions `start..end`.
fn slice_spans(spans: &[Span], start: usize, end: usize) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = 0_usize;
    for span in spans {
        let len = span.text.chars().count();
        let from = at.max(start);
        let to = (at + len).min(end);
        if from < to {
            out.push(Span {
                text: span.text.chars().skip(from - at).take(to - from).collect(),
                attributes: span.attributes.clone(),
            });
        }
        at += len;
    }
    out
}

/// Carrying spans into another body re-asserts their attributes as the COMPLETE
/// desired set, so a key the previous span left inherited is stripped and a key
/// the two share costs one mark row rather than one per span.
fn carry(spans: &[Span]) -> Vec<DeltaOp> {
    spans
        .iter()
        .map(|span| DeltaOp::Insert {
            insert: span.text.clone(),
            attributes: Some(
                span.attributes
                    .iter()
                    .map(|(key, value)| (key.clone(), Some(value.clone())))
                    .collect::<Attrs>(),
            ),
        })
        .collect()
}

/// Every key the carried spans hold, checked against the schema before the
/// first write, so a split or a merge that cannot complete stores nothing.
fn declared_keys<Sc: MarkSchema>(spans: &[Span]) -> Result<(), StoreError> {
    for span in spans {
        for key in span.attributes.keys() {
            let _ignored = expand_for::<Sc>(key, false)?;
        }
    }
    Ok(())
}

const fn retain(count: usize) -> DeltaOp {
    DeltaOp::Retain {
        retain: count,
        attributes: None,
    }
}

fn invalid(message: &str) -> StoreError {
    StoreError::StorageError(crate::interface::StorageError::InvalidData(message.into()))
}

fn out_of_bounds(pos: usize) -> StoreError {
    invalid(&format!("position {pos} out of bounds"))
}

fn unknown_block(block: BlockId) -> StoreError {
    invalid(&format!("unknown block {:?}", block.0))
}

#[cfg(test)]
mod tests {
    use super::{Anchor, Bias, BlockId, Prop, RichDocument};
    use crate::collections::DefaultMarks;
    use crate::collections::Root;
    use crate::env;
    use crate::store::MainStorage;

    type Doc = RichDocument<DefaultMarks, MainStorage>;

    const ABSENT: BlockId = BlockId((0xF0, 7)); // names a spine character nobody minted

    fn doc() -> Root<Doc> {
        env::reset_for_testing();
        Root::new(Doc::new)
    }

    /// The rendered order, which is the only thing the corner table is about.
    fn order(doc: &Doc) -> Vec<BlockId> {
        doc.blocks()
            .unwrap()
            .into_iter()
            .map(|view| view.id)
            .collect()
    }

    fn place(doc: &mut Doc, block: BlockId, anchor: Anchor) {
        doc.row(block).unwrap().set(Prop::Place(anchor)).unwrap();
    }

    #[test]
    fn insert_block__orders_by_the_spine() {
        let mut doc = doc();
        let first = doc.insert_block(None, "paragraph", 0).unwrap();
        let last = doc.insert_block(Some(first), "paragraph", 0).unwrap();
        let middle = doc.insert_block(Some(first), "heading", 1).unwrap();
        let top = doc.insert_block(None, "paragraph", 0).unwrap();

        assert_eq!(order(&doc), vec![top, first, middle, last]);
        let kinds: Vec<String> = doc
            .blocks()
            .unwrap()
            .into_iter()
            .map(|view| view.kind)
            .collect();
        assert_eq!(kinds, ["paragraph", "paragraph", "heading", "paragraph"]);
    }

    #[test]
    fn two_blocks_naming_one_spine_character_both_render_ordered_by_id() {
        let mut doc = doc();
        let first = doc.insert_block(None, "paragraph", 0).unwrap();
        let second = doc.insert_block(Some(first), "paragraph", 0).unwrap();
        place(
            &mut doc,
            second,
            Anchor::Char {
                id: first.0,
                bias: Bias::Before,
            },
        );

        let mut expected = vec![first, second];
        expected.sort_unstable();
        assert_eq!(order(&doc), expected, "the tie must break on the block id");
    }

    #[test]
    fn a_block_naming_a_tombstoned_spine_character_renders_at_the_gap() {
        let mut doc = doc();
        let first = doc.insert_block(None, "paragraph", 0).unwrap();
        let second = doc.insert_block(Some(first), "paragraph", 0).unwrap();
        let third = doc.insert_block(Some(second), "paragraph", 0).unwrap();

        let _removed = doc.spine.delete_range(1, 2).unwrap();
        assert_eq!(
            order(&doc),
            vec![first, second, third],
            "a deleted slot names the gap it left, so nothing moves"
        );
    }

    #[test]
    fn a_block_naming_an_unknown_spine_character_renders_at_the_end() {
        let mut doc = doc();
        let first = doc.insert_block(None, "paragraph", 0).unwrap();
        let second = doc.insert_block(Some(first), "paragraph", 0).unwrap();
        let third = doc.insert_block(Some(second), "paragraph", 0).unwrap();
        place(
            &mut doc,
            first,
            Anchor::Char {
                id: ABSENT.0,
                bias: Bias::Before,
            },
        );

        assert_eq!(
            order(&doc),
            vec![second, third, first],
            "content that exists must never be invisible"
        );
    }

    /// The same code path as the unknown character: a row can arrive first.
    #[test]
    fn unplaced_blocks_order_among_themselves_by_id() {
        let mut doc = doc();
        let first = doc.insert_block(None, "paragraph", 0).unwrap();
        let second = doc.insert_block(Some(first), "paragraph", 0).unwrap();
        let third = doc.insert_block(Some(second), "paragraph", 0).unwrap();
        for block in [third, first] {
            place(
                &mut doc,
                block,
                Anchor::Char {
                    id: ABSENT.0,
                    bias: Bias::Before,
                },
            );
        }

        let mut unplaced = vec![first, third];
        unplaced.sort_unstable();
        assert_eq!(order(&doc), [vec![second], unplaced].concat());
    }

    #[test]
    fn a_spine_character_no_block_names_is_invisible() {
        let mut doc = doc();
        let first = doc.insert_block(None, "paragraph", 0).unwrap();
        let second = doc.insert_block(Some(first), "paragraph", 0).unwrap();
        doc.move_block(first, Some(second)).unwrap();

        assert_eq!(doc.spine.len().unwrap(), 3, "the old slot is left live");
        assert_eq!(order(&doc), vec![second, first]);
    }

    #[test]
    fn a_place_at_a_document_edge_resolves_without_a_special_case() {
        let mut doc = doc();
        let first = doc.insert_block(None, "paragraph", 0).unwrap();
        let second = doc.insert_block(Some(first), "paragraph", 0).unwrap();
        let third = doc.insert_block(Some(second), "paragraph", 0).unwrap();

        place(&mut doc, first, Anchor::End);
        place(&mut doc, third, Anchor::Start);
        assert_eq!(
            order(&doc),
            vec![third, second, first],
            "`Start` is position 0 and `End` is the spine length, with no arm of their own"
        );
    }

    #[test]
    fn a_deleted_block_is_skipped_but_its_rows_stay() {
        let mut doc = doc();
        let first = doc.insert_block(None, "paragraph", 0).unwrap();
        let second = doc.insert_block(Some(first), "paragraph", 0).unwrap();
        let _undo = doc.apply_delta(second, &[insert("gone")]).unwrap();

        assert!(!doc.delete_block(second).unwrap(), "it was not deleted yet");
        assert!(doc.delete_block(second).unwrap(), "the second call sees it");

        assert_eq!(order(&doc), vec![first]);
        assert_eq!(doc.block(second).unwrap(), None);
        assert_eq!(
            doc.block_delta(second).unwrap()[0].text,
            "gone",
            "a tombstone hides the block, it does not drop its body"
        );
    }

    fn insert(text: &str) -> super::DeltaOp {
        super::DeltaOp::Insert {
            insert: text.to_owned(),
            attributes: None,
        }
    }
}
