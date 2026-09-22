//! `RichText` - a [`FugueText`] plus Peritext-style formatting marks.
//!
//! A mark is a row in a sibling map keyed by [`MarkId`], never anything inside a
//! `TextBlock`, so the text layer's join and its frozen-run immutability are
//! untouched. A row is written ONCE and never rewritten: removing formatting is
//! a new row with a greater id and `value: None`. Two replicas holding one id
//! therefore hold byte-identical values, which is why this composite needs no
//! `CrdtType` of its own and cannot reconcile wrong.
//!
//! Reading is one rule: per character, per key, the covering mark with the
//! greatest `MarkId` wins. Where a mark's range GROWS when text is typed at its
//! edge is decided once, at write time, by the two anchor biases the app's
//! [`MarkSchema`] picks; a read never asks the schema anything, so replicas
//! running different app versions still render identical spans.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize, Serializer};

use super::error::StoreError;
use super::fugue::{FugueTree, RawId};
use super::fugue_text::{
    anchor_at_in, local_replica, Anchor, Bias, FugueText, IdRange, PositionIndex, Removed,
};
use super::mark_schema::{expand_for, mark_prefix, Expand, MarkSchema};
use super::{CrdtType, UnorderedMap};
use crate::env;
use crate::store::{MainStorage, StorageAdaptor};

const MARKS_FIELD: &str = "__rich_marks"; // child id namespace for the mark map
const LAMPORT_EXHAUSTED: &str = "mark lamport space exhausted";

/// A Lamport-ordered, globally unique mark identity.
///
/// `lamport` is one greater than the greatest this replica can see at write
/// time; ties break on `replica`. Field order is the comparator, so the derived
/// `Ord` IS the rule "greater counter wins, greater node id breaks the tie".
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
pub struct MarkId {
    pub lamport: u64,
    pub replica: u64,
}

/// Storage key for a mark; owns its serialized bytes for `AsRef<[u8]>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkKey {
    id: MarkId,
    bytes: Vec<u8>,
}

impl MarkKey {
    fn new(id: MarkId) -> Self {
        // `MarkId` is fixed-size POD, so serialization cannot fail.
        let bytes = borsh::to_vec(&id).unwrap_or_default();
        Self { id, bytes }
    }
}

impl BorshSerialize for MarkKey {
    fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
        BorshSerialize::serialize(&self.id, writer)
    }
}

impl BorshDeserialize for MarkKey {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        let id = MarkId::deserialize_reader(reader)?;
        Ok(Self::new(id))
    }
}

impl AsRef<[u8]> for MarkKey {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// One formatting operation, written once and never rewritten.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Mark {
    /// Duplicated from the map key so a row read in isolation is self-describing.
    pub id: MarkId,
    pub start: Anchor,
    pub end: Anchor,
    pub key: String,
    /// `None` removes `key` over the span.
    pub value: Option<String>,
}

/// A requested attribute set. `None` as a value removes the key.
pub type Attrs = BTreeMap<String, Option<String>>;

/// One step of an attributed editor change, walking the document as it was
/// before the change. JSON is Quill's: `{"retain": 3, "attributes": {...}}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeltaOp {
    Retain {
        retain: usize,
        #[serde(default, skip_serializing_if = "Option::is_none", with = "attrs_json")]
        attributes: Option<Attrs>,
    },
    Insert {
        insert: String,
        #[serde(default, skip_serializing_if = "Option::is_none", with = "attrs_json")]
        attributes: Option<Attrs>,
    },
    Delete {
        delete: usize,
    },
}

/// A rendered run: `text`, never offsets, because a run-length list cannot
/// disagree with itself and offsets go stale.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    /// Absent key = not set. There is no null in the output.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        with = "span_attrs_json"
    )]
    pub attributes: BTreeMap<String, String>,
}

/// The inverse of one [`RichText::apply_delta`], in the order it must be replayed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaUndo(pub Vec<UndoStep>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UndoStep {
    /// Delete the characters this delta minted. By id, not by index, so a peer's
    /// concurrent edit inside the range cannot misdirect it.
    Delete(IdRange),
    /// Re-insert text this delta deleted, then re-apply the attribute runs it
    /// carried. `attrs` offsets are char offsets INTO `removed.text`, so they
    /// name the newly minted characters rather than the tombstoned originals.
    Insert {
        removed: Removed,
        attrs: Vec<AttrRun>,
    },
    /// Restore a key's prior value over a range that still exists.
    Mark {
        start: Anchor,
        end: Anchor,
        key: String,
        value: Option<String>,
    },
}

/// One maximal run over which every attribute was constant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttrRun {
    pub start: usize,
    pub end: usize,
    pub attributes: BTreeMap<String, String>,
}

/// Text plus Peritext-style formatting marks.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct RichText<Sc: MarkSchema, S: StorageAdaptor = MainStorage> {
    #[borsh(bound(serialize = "", deserialize = ""))]
    text: FugueText<S>,
    #[borsh(bound(serialize = "", deserialize = ""))]
    marks: UnorderedMap<MarkKey, Mark, S>,
    /// `fn() -> Sc` rather than `Sc`, so the schema decides nothing about this
    /// type's auto traits or variance. Never reaches a stored byte.
    #[borsh(skip)]
    schema: PhantomData<fn() -> Sc>,
}

/// Re-key both child collections under the storage parent so a nested document converges.
impl<Sc: MarkSchema, S: StorageAdaptor> super::rekey::RekeyTarget for RichText<Sc, S> {
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        self.text.rekey_relative_to(parent_id);
        self.marks
            .reassign_deterministic_id_under(parent_id, MARKS_FIELD, CrdtType::UnorderedMap);
    }
}

impl<Sc: MarkSchema> RichText<Sc, MainStorage> {
    /// Create a new empty document with a random id.
    #[must_use]
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Create a new document with a deterministic id derived from `field_name`.
    #[must_use]
    pub fn new_with_field_name(field_name: &str) -> Self {
        let doc = Self {
            text: FugueText::new_with_field_name(field_name),
            marks: UnorderedMap::new_with_field_name_and_crdt_type(
                None,
                &format!("{field_name}{MARKS_FIELD}"),
                CrdtType::UnorderedMap,
            ),
            schema: PhantomData,
        };
        let _ignored = super::rekey::register_rekey::<Self>();
        doc
    }
}

impl<Sc: MarkSchema> Default for RichText<Sc, MainStorage> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Sc: MarkSchema, S: StorageAdaptor> RichText<Sc, S> {
    pub(super) fn new_internal() -> Self {
        let doc = Self {
            text: FugueText::new_internal(),
            marks: UnorderedMap::new_internal(),
            schema: PhantomData,
        };
        let _ignored = super::rekey::register_rekey::<Self>();
        doc
    }

    /// Called by the `#[app::state]` macro after `init()`.
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        self.text.reassign_deterministic_id(field_name);
        self.marks
            .reassign_deterministic_id(&format!("{field_name}{MARKS_FIELD}"));
        self.marks.set_collection_crdt_type(CrdtType::UnorderedMap);
    }

    // ---- writes ----

    /// Apply one editor transaction, text and formatting together, and return
    /// the steps that undo it in replay order.
    ///
    /// Every fallible check runs before the first write, so a delta that cannot
    /// complete stores nothing at all.
    pub fn apply_delta(&mut self, ops: &[DeltaOp]) -> Result<DeltaUndo, StoreError> {
        self.apply_delta_with_replica(ops, minting_replica("apply_delta"))
    }

    /// Set `key` to `value` over visible positions `start..end`.
    ///
    /// `Ok(None)` means the write was SKIPPED as redundant: every character in
    /// the range already resolves to `value` for `key`.
    pub fn mark(
        &mut self,
        start: usize,
        end: usize,
        key: &str,
        value: Option<&str>,
    ) -> Result<Option<MarkId>, StoreError> {
        self.mark_with_replica(start, end, key, value, minting_replica("mark"))
    }

    /// [`Self::mark`] with `value: None`. Readability only; it takes the same
    /// path, so the two can never disagree about inverted expand.
    pub fn unmark(
        &mut self,
        start: usize,
        end: usize,
        key: &str,
    ) -> Result<Option<MarkId>, StoreError> {
        self.mark(start, end, key, None)
    }

    /// For a caller that already holds anchors: an undo, a stored comment range,
    /// a split carrying formatting. Bypasses the schema, whose only job is to
    /// choose biases that are already decided here.
    pub fn mark_at(
        &mut self,
        start: Anchor,
        end: Anchor,
        key: &str,
        value: Option<&str>,
    ) -> Result<Option<MarkId>, StoreError> {
        self.mark_at_with_replica(start, end, key, value, minting_replica("mark_at"))
    }

    /// The migration-safe minting variant: `replica` is given rather than read
    /// from the node-local device id.
    pub fn mark_with_replica(
        &mut self,
        start: usize,
        end: usize,
        key: &str,
        value: Option<&str>,
        replica: u64,
    ) -> Result<Option<MarkId>, StoreError> {
        if start > end {
            return Err(invalid("start must be <= end"));
        }
        let expand = expand_for::<Sc>(key, value.is_none())?;
        let tree = self.text.tree()?;
        if end > tree.len() {
            return Err(out_of_bounds(end));
        }
        if start == end {
            return Ok(None);
        }

        let marks = self.sorted_marks()?;
        let index = PositionIndex::build(&tree);
        if uniform_value(&runs_of(&index, &marks, tree.len()), key, value, start, end) {
            return Ok(None);
        }

        let (start_anchor, end_anchor) = anchor_pair(&tree, start, end, expand)?;
        self.put_mark(&marks, start_anchor, end_anchor, key, value, replica)
            .map(Some)
    }

    /// Replay a [`DeltaUndo`] in order, returning the undo of the undo, so redo
    /// needs no extra machinery.
    pub fn apply_undo(&mut self, undo: &DeltaUndo) -> Result<DeltaUndo, StoreError> {
        let replica = minting_replica("apply_undo");
        let mut redo: Vec<UndoStep> = Vec::new();
        for step in &undo.0 {
            match *step {
                UndoStep::Delete(ref ids) => {
                    if let Some(removed) = self.text.delete_ids(ids)? {
                        redo.push(UndoStep::Insert {
                            removed,
                            attrs: Vec::new(),
                        });
                    }
                }
                UndoStep::Insert {
                    ref removed,
                    ref attrs,
                } => {
                    if let Some(minted) = self.text.insert_str_at(&removed.anchor, &removed.text)? {
                        self.restore_runs(&minted, attrs, replica)?;
                        redo.push(UndoStep::Delete(minted));
                    }
                }
                UndoStep::Mark {
                    start,
                    end,
                    ref key,
                    ref value,
                } => {
                    redo.extend(self.prior_marks_at(start, end, key)?);
                    let _ignored =
                        self.mark_at_with_replica(start, end, key, value.as_deref(), replica)?;
                }
            }
        }
        redo.reverse();
        Ok(DeltaUndo(redo))
    }

    // ---- reads ----

    /// The renderable document: maximal run-length spans, tombstones excluded,
    /// no null values. One tree rebuild, one batched anchor resolution, one sweep.
    pub fn to_delta(&self) -> Result<Vec<Span>, StoreError> {
        let tree = self.text.tree()?;
        let index = PositionIndex::build(&tree);
        let marks = self.sorted_marks()?;
        let chars: Vec<char> = tree.values().chars().collect();

        let mut out: Vec<Span> = Vec::new();
        for run in runs_of(&index, &marks, chars.len()) {
            let text: String = chars
                .get(run.start..run.end)
                .unwrap_or_default()
                .iter()
                .collect();
            push_span(&mut out, text, run.attributes);
        }
        Ok(out)
    }

    /// The raw rows, ascending by [`MarkId`]. Diagnostics and undo stacks.
    pub fn marks(&self) -> Result<Vec<Mark>, StoreError> {
        self.sorted_marks()
    }

    pub fn get_text(&self) -> Result<String, StoreError> {
        self.text.get_text()
    }

    pub fn text_range(&self, start: usize, end: usize) -> Result<String, StoreError> {
        self.text.text_range(start, end)
    }

    pub fn len(&self) -> Result<usize, StoreError> {
        self.text.len()
    }

    pub fn is_empty(&self) -> Result<bool, StoreError> {
        self.text.is_empty()
    }

    pub fn anchor_at(&self, pos: usize, bias: Bias) -> Result<Anchor, StoreError> {
        self.text.anchor_at(pos, bias)
    }

    /// The only resolver: one tree rebuild for the whole slice. Resolving a mark
    /// at a time is `O(N * m)`, which is a trap at document scale, so the fast
    /// path is the only path. `None` is an anchor this replica cannot place yet.
    pub fn resolve_many(&self, anchors: &[Anchor]) -> Result<Vec<Option<usize>>, StoreError> {
        let index = PositionIndex::build(&self.text.tree()?);
        Ok(anchors.iter().map(|anchor| index.resolve(anchor)).collect())
    }

    // ---- internals ----

    fn apply_delta_with_replica(
        &mut self,
        ops: &[DeltaOp],
        replica: u64,
    ) -> Result<DeltaUndo, StoreError> {
        self.validate(ops)?;

        let mut undo: Vec<UndoStep> = Vec::new();
        let mut pos = 0_usize;
        for op in ops {
            match *op {
                DeltaOp::Retain {
                    retain,
                    ref attributes,
                } => {
                    if let Some(attrs) = attributes {
                        undo.extend(self.retain_attrs(pos, retain, attrs, replica)?);
                    }
                    pos += retain;
                }
                DeltaOp::Insert {
                    ref insert,
                    ref attributes,
                } => {
                    if let Some(minted) = self.insert_at(pos, insert, replica)? {
                        undo.push(UndoStep::Delete(minted));
                        if let Some(requested) = attributes {
                            self.reconcile_insert(&minted, requested, replica)?;
                        }
                    }
                    pos += insert.chars().count();
                }
                DeltaOp::Delete { delete } => {
                    let end = (pos + delete).min(self.text.len()?);
                    let attrs = self.attr_runs(pos, end)?;
                    if let Some(removed) = self.text.delete_range(pos, end)? {
                        undo.push(UndoStep::Insert { removed, attrs });
                    }
                }
            }
        }
        undo.reverse();
        Ok(DeltaUndo(undo))
    }

    /// Everything a delta can fail on, against a pure length walk. Writing only
    /// after this is what lets a rejected delta leave zero rows behind, across
    /// two collections that share no draft.
    fn validate(&self, ops: &[DeltaOp]) -> Result<(), StoreError> {
        let mut len = self.text.len()?;
        let mut pos = 0_usize;
        for op in ops {
            match *op {
                DeltaOp::Retain {
                    retain,
                    ref attributes,
                } => {
                    pos = advance(pos, retain)?;
                    if let Some(attrs) = attributes {
                        for (key, value) in attrs {
                            let _ignored = expand_for::<Sc>(key, value.is_none())?;
                        }
                        if pos > len {
                            return Err(out_of_bounds(pos));
                        }
                    }
                }
                DeltaOp::Insert {
                    ref insert,
                    ref attributes,
                } => {
                    if pos > len {
                        return Err(out_of_bounds(pos));
                    }
                    for (key, value) in attributes.iter().flatten() {
                        let _ignored = expand_for::<Sc>(key, value.is_none())?;
                    }
                    let count = insert.chars().count();
                    len = advance(len, count)?;
                    pos = advance(pos, count)?;
                }
                // A delete past the end is a silent clamp, matching `FugueText`.
                DeltaOp::Delete { delete } => len -= delete.min(len - pos.min(len)),
            }
        }
        Ok(())
    }

    /// Peritext's tombstone rule, read off STORED ANCHOR SIDES rather than the
    /// schema, so every replica places a boundary insert identically.
    fn insert_at(
        &mut self,
        pos: usize,
        text: &str,
        replica: u64,
    ) -> Result<Option<IdRange>, StoreError> {
        if text.is_empty() {
            return Ok(None);
        }
        let tree = self.text.tree()?;
        let marks = self.sorted_marks()?;
        match boundary_left_origin(&tree, &marks, pos) {
            Some(left) => self.text.insert_str_after(Some(left), replica, text),
            None => self.text.insert_str_with_replica(pos, replica, text),
        }
    }

    /// A `Retain` carrying attributes: one `mark` per key, in `BTreeMap` order,
    /// so the ids one delta mints are a deterministic function of the delta.
    fn retain_attrs(
        &mut self,
        pos: usize,
        retain: usize,
        attrs: &Attrs,
        replica: u64,
    ) -> Result<Vec<UndoStep>, StoreError> {
        let mut undo = Vec::new();
        for (key, value) in attrs {
            let prior = self.prior_marks(pos, pos + retain, key)?;
            if self
                .mark_with_replica(pos, pos + retain, key, value.as_deref(), replica)?
                .is_some()
            {
                undo.extend(prior);
            }
        }
        Ok(undo)
    }

    /// `attributes` present is the COMPLETE desired set for the inserted text, so
    /// a key it omits is stripped even where the new characters inherited it.
    fn reconcile_insert(
        &mut self,
        minted: &IdRange,
        requested: &Attrs,
        replica: u64,
    ) -> Result<(), StoreError> {
        let Some((start, end)) = self.minted_range(minted)? else {
            return Ok(());
        };
        // Minted characters are contiguous and land in one gap, so no older
        // anchor can fall between them: they all inherit the same set.
        let inherited = self
            .attr_runs(start, end)?
            .into_iter()
            .next()
            .map(|run| run.attributes)
            .unwrap_or_default();

        let keys: BTreeSet<&String> = requested.keys().chain(inherited.keys()).collect();
        for key in keys {
            let want = requested.get(key).cloned().flatten();
            let _ignored = self.mark_with_replica(start, end, key, want.as_deref(), replica)?;
        }
        Ok(())
    }

    /// Re-apply the attribute runs a delete carried, over the characters the undo
    /// just minted. The originals stay tombstoned forever, so restoring by run is
    /// explicit rather than a bet on where Fugue placed the new nodes.
    fn restore_runs(
        &mut self,
        minted: &IdRange,
        attrs: &[AttrRun],
        replica: u64,
    ) -> Result<(), StoreError> {
        let Some((base, end)) = self.minted_range(minted)? else {
            return Ok(());
        };
        for run in attrs {
            for (key, value) in &run.attributes {
                let _ignored = self.mark_with_replica(
                    (base + run.start).min(end),
                    (base + run.end).min(end),
                    key,
                    Some(value.as_str()),
                    replica,
                )?;
            }
        }
        Ok(())
    }

    /// The visible half-open range `minted` occupies now, or `None` once every
    /// character it named has been deleted by a peer.
    fn minted_range(&self, minted: &IdRange) -> Result<Option<(usize, usize)>, StoreError> {
        let index = PositionIndex::build(&self.text.tree()?);
        let start = index.resolve(&Anchor::Char {
            id: minted.start,
            bias: Bias::Before,
        });
        let end = index.resolve(&Anchor::Char {
            id: last_id(minted),
            bias: Bias::After,
        });
        Ok(match (start, end) {
            (Some(start), Some(end)) if start < end => Some((start, end)),
            _ => None,
        })
    }

    fn mark_at_with_replica(
        &mut self,
        start: Anchor,
        end: Anchor,
        key: &str,
        value: Option<&str>,
        replica: u64,
    ) -> Result<Option<MarkId>, StoreError> {
        let _ignored = mark_prefix(key)?;
        let tree = self.text.tree()?;
        let index = PositionIndex::build(&tree);
        let marks = self.sorted_marks()?;

        if let (Some(from), Some(to)) = (index.resolve(&start), index.resolve(&end)) {
            if from < to
                && uniform_value(&runs_of(&index, &marks, tree.len()), key, value, from, to)
            {
                return Ok(None);
            }
        }
        self.put_mark(&marks, start, end, key, value, replica)
            .map(Some)
    }

    /// The prior value of `key` over `start..end`, as the `UndoStep::Mark` list
    /// that restores it: one step per maximal run the value was constant over.
    fn prior_marks(
        &self,
        start: usize,
        end: usize,
        key: &str,
    ) -> Result<Vec<UndoStep>, StoreError> {
        let tree = self.text.tree()?;
        let index = PositionIndex::build(&tree);
        let marks = self.sorted_marks()?;
        let end = end.min(tree.len());

        let mut steps = Vec::new();
        for (from, to, value) in key_runs(&runs_of(&index, &marks, tree.len()), key, start, end) {
            // The boundary policy is frozen into the token here, so replaying it
            // restores what was there rather than what a later schema prefers.
            let expand = expand_for::<Sc>(key, value.is_none())?;
            let (start_anchor, end_anchor) = anchor_pair(&tree, from, to, expand)?;
            steps.push(UndoStep::Mark {
                start: start_anchor,
                end: end_anchor,
                key: key.to_owned(),
                value,
            });
        }
        Ok(steps)
    }

    fn prior_marks_at(
        &self,
        start: Anchor,
        end: Anchor,
        key: &str,
    ) -> Result<Vec<UndoStep>, StoreError> {
        let index = PositionIndex::build(&self.text.tree()?);
        match (index.resolve(&start), index.resolve(&end)) {
            (Some(from), Some(to)) if from < to => self.prior_marks(from, to, key),
            _ => Ok(Vec::new()),
        }
    }

    /// Every key's value over `start..end`, as maximal runs with offsets
    /// RELATIVE to `start`, which is what an undo token carries.
    fn attr_runs(&self, start: usize, end: usize) -> Result<Vec<AttrRun>, StoreError> {
        let tree = self.text.tree()?;
        let index = PositionIndex::build(&tree);
        let marks = self.sorted_marks()?;
        let end = end.min(tree.len());

        let mut out: Vec<AttrRun> = Vec::new();
        for run in runs_of(&index, &marks, tree.len()) {
            let from = run.start.max(start);
            let to = run.end.min(end);
            if from >= to {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.attributes == run.attributes => last.end = to - start,
                _ => out.push(AttrRun {
                    start: from - start,
                    end: to - start,
                    attributes: run.attributes,
                }),
            }
        }
        Ok(out)
    }

    fn put_mark(
        &mut self,
        marks: &[Mark],
        start: Anchor,
        end: Anchor,
        key: &str,
        value: Option<&str>,
        replica: u64,
    ) -> Result<MarkId, StoreError> {
        let lamport = match marks.iter().map(|mark| mark.id.lamport).max() {
            Some(top) => top
                .checked_add(1)
                .ok_or_else(|| invalid(LAMPORT_EXHAUSTED))?,
            None => 1,
        };
        let id = MarkId { lamport, replica };
        let _ignored = self.marks.insert(
            MarkKey::new(id),
            Mark {
                id,
                start,
                end,
                key: key.to_owned(),
                value: value.map(str::to_owned),
            },
        )?;
        Ok(id)
    }

    fn sorted_marks(&self) -> Result<Vec<Mark>, StoreError> {
        let mut marks: Vec<Mark> = self.marks.entries()?.map(|(_, mark)| mark).collect();
        marks.sort_by_key(|mark| mark.id);
        Ok(marks)
    }

    /// Copy in `other`'s mark rows. Write-once makes the join the identity, so a
    /// key both sides hold needs no reconciliation at all.
    pub(crate) fn merge_marks_from<S2: StorageAdaptor>(
        &mut self,
        other: &RichText<Sc, S2>,
    ) -> Result<(), StoreError> {
        for (key, incoming) in other.marks.entries()? {
            if self.marks.get(&key)?.is_some() {
                continue;
            }
            // A storage tombstone means `self` removed it: do not resurrect.
            if crate::index::Index::<S>::is_deleted(self.marks.entry_id(&key))? {
                continue;
            }
            let _ignored = self.marks.insert(key, incoming)?;
        }
        Ok(())
    }

    pub(crate) fn merge_from<S2: StorageAdaptor>(
        &mut self,
        other: &RichText<Sc, S2>,
    ) -> Result<(), StoreError> {
        self.text.merge_blocks_from(&other.text)?;
        self.merge_marks_from(other)
    }
}

/// A mark that this replica can place, as visible positions.
struct Active<'a> {
    start: usize,
    end: usize,
    id: MarkId,
    key: &'a str,
    value: Option<&'a str>,
}

/// One maximal run of characters whose winning mark set is constant.
struct Run {
    start: usize,
    end: usize,
    attributes: BTreeMap<String, String>,
}

/// Marks whose anchors both resolve here and that cover at least one character.
///
/// A row that resolves nowhere is RETAINED and skipped for this read: dropping
/// it would diverge from a replica that already holds the text, and it becomes
/// active, with the same outcome, the moment that text arrives. A row whose
/// start lands at or after its end is hostile input and covers nothing.
fn active_marks<'a>(index: &PositionIndex, marks: &'a [Mark]) -> Vec<Active<'a>> {
    marks
        .iter()
        .filter_map(|mark| {
            let start = index.resolve(&mark.start)?;
            let end = index.resolve(&mark.end)?;
            (start < end).then_some(Active {
                start,
                end,
                id: mark.id,
                key: mark.key.as_str(),
                value: mark.value.as_deref(),
            })
        })
        .collect()
}

/// The sweep: between two event positions the open set is exactly the marks
/// covering those characters.
///
/// A run is emitted BEFORE the batch at its right edge is applied, and an active
/// mark covers at least one character, so no slot opens and closes at one
/// position. Intra-batch order therefore cannot change the result; it is fixed
/// only so the sort is total.
fn runs_of(index: &PositionIndex, marks: &[Mark], len: usize) -> Vec<Run> {
    const CLOSE: u8 = 0;
    const OPEN: u8 = 1;

    let active = active_marks(index, marks);
    let mut events: Vec<(usize, u8, usize)> = Vec::with_capacity(active.len() * 2);
    for (slot, mark) in active.iter().enumerate() {
        events.push((mark.end, CLOSE, slot));
        events.push((mark.start, OPEN, slot));
    }
    events.sort_unstable();

    let mut runs: Vec<Run> = Vec::new();
    let mut open: BTreeSet<usize> = BTreeSet::new();
    let mut cursor = 0_usize;
    let mut at = 0_usize;
    while let Some(&(pos, _, _)) = events.get(at) {
        if pos > cursor {
            runs.push(Run {
                start: cursor,
                end: pos,
                attributes: attrs_of(&open, &active),
            });
            cursor = pos;
        }
        while let Some(&(_, kind, slot)) = events.get(at).filter(|event| event.0 == pos) {
            if kind == CLOSE {
                let _ignored = open.remove(&slot);
            } else {
                let _ignored = open.insert(slot);
            }
            at += 1;
        }
    }
    if cursor < len {
        runs.push(Run {
            start: cursor,
            end: len,
            attributes: attrs_of(&open, &active),
        });
    }
    runs
}

/// Per key, the covering mark with the greatest `MarkId` wins, independently of
/// every other key. That independence is what makes bold and italic coexist
/// while two colours do not, and it is what makes `comment:a` and `comment:b`
/// both survive with no allow-multiple flag anywhere.
fn attrs_of(open: &BTreeSet<usize>, active: &[Active<'_>]) -> BTreeMap<String, String> {
    let mut best: BTreeMap<&str, (MarkId, Option<&str>)> = BTreeMap::new();
    for slot in open {
        let Some(mark) = active.get(*slot) else {
            continue;
        };
        let winner = best.entry(mark.key).or_insert((mark.id, mark.value));
        if mark.id > winner.0 {
            *winner = (mark.id, mark.value);
        }
    }
    best.into_iter()
        .filter_map(|(key, (_, value))| value.map(|value| (key.to_owned(), value.to_owned())))
        .collect()
}

/// Whether every character in `start..end` already resolves to `value` for `key`.
fn uniform_value(runs: &[Run], key: &str, value: Option<&str>, start: usize, end: usize) -> bool {
    runs.iter()
        .filter(|run| run.end > start && run.start < end)
        .all(|run| run.attributes.get(key).map(String::as_str) == value)
}

/// One key's value over `start..end`, coalesced into maximal runs.
fn key_runs(
    runs: &[Run],
    key: &str,
    start: usize,
    end: usize,
) -> Vec<(usize, usize, Option<String>)> {
    let mut out: Vec<(usize, usize, Option<String>)> = Vec::new();
    for run in runs {
        let from = run.start.max(start);
        let to = run.end.min(end);
        if from >= to {
            continue;
        }
        let value = run.attributes.get(key).cloned();
        match out.last_mut() {
            Some(last) if last.2 == value => last.1 = to,
            _ => out.push((from, to, value)),
        }
    }
    out
}

/// Expand IS the pair of biases, and nothing else.
///
/// A growing start is anchored AFTER the character before the range, so a later
/// insert at that gap resolves inside it; a non-growing start is anchored BEFORE
/// the range's first character, so the same insert resolves outside. The end is
/// the mirror. `anchor_at_in` already collapses both document edges, which
/// always grow, to `Anchor::Start` / `Anchor::End`.
fn anchor_pair(
    tree: &FugueTree,
    start: usize,
    end: usize,
    expand: Expand,
) -> Result<(Anchor, Anchor), StoreError> {
    let start_bias = if expand.expands_before() {
        Bias::After
    } else {
        Bias::Before
    };
    let end_bias = if expand.expands_after() {
        Bias::Before
    } else {
        Bias::After
    };
    Ok((
        anchor_at_in(tree, start, start_bias)?,
        anchor_at_in(tree, end, end_bias)?,
    ))
}

/// The node a boundary insert at `pos` must directly follow, if any.
///
/// Peritext: scan the tombstones at this position, and if any carries the `After`
/// anchor of a formatting operation, insert after the LAST such tombstone. That
/// is what keeps typing at the end of a deleted link out of the link while the
/// same edit at the end of a deleted bold run stays bold.
fn boundary_left_origin(tree: &FugueTree, marks: &[Mark], pos: usize) -> Option<RawId> {
    let anchored: BTreeSet<RawId> = marks
        .iter()
        .flat_map(|mark| [mark.start, mark.end])
        .filter_map(|anchor| match anchor {
            Anchor::Char {
                id,
                bias: Bias::After,
            } => Some(id),
            _ => None,
        })
        .collect();

    let mut live = 0_usize;
    let mut last = None;
    for id in tree.order().into_iter().flatten() {
        let Some(node) = tree.node(id) else { continue };
        if node.value.is_some() {
            if live == pos {
                break;
            }
            live += 1;
        } else if live == pos && anchored.contains(&id) {
            last = Some(id);
        }
    }
    last
}

/// Appends `text` to the last span when the attributes match, which is what
/// makes `to_delta` a function of the rendered document rather than of which
/// losing marks a replica happens to hold.
fn push_span(out: &mut Vec<Span>, text: String, attributes: BTreeMap<String, String>) {
    if text.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(last) if last.attributes == attributes => last.text.push_str(&text),
        _ => out.push(Span { text, attributes }),
    }
}

const fn last_id(range: &IdRange) -> RawId {
    (range.start.0, range.start.1 + range.len - 1)
}

fn advance(pos: usize, count: usize) -> Result<usize, StoreError> {
    pos.checked_add(count).ok_or_else(|| out_of_bounds(pos))
}

/// The local replica, for a call that mints mark ids; a loud panic beats a
/// silent network divergence.
#[expect(
    clippy::panic,
    reason = "minting from the node-local device id inside a migration diverges ids across nodes"
)]
fn minting_replica(method: &str) -> u64 {
    if env::in_merge_mode() {
        panic!(
            "RichText::{method}() is non-deterministic during a state migration: it mints mark \
             ids from the node-local device id, diverging ids across nodes. Seed with \
             `mark_with_replica(start, end, key, value, replica)`."
        );
    }
    local_replica()
}

pub(super) fn invalid(message: &str) -> StoreError {
    StoreError::StorageError(crate::interface::StorageError::InvalidData(message.into()))
}

pub(super) fn out_of_bounds(pos: usize) -> StoreError {
    invalid(&format!("position {pos} out of bounds"))
}

/// A JSON attribute value, before canonicalisation to a string.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawAttr {
    Bool(bool),
    Unsigned(u64),
    Signed(i64),
    Float(f64),
    Text(String),
}

impl RawAttr {
    fn canonical(self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Unsigned(value) => value.to_string(),
            Self::Signed(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Text(value) => value,
        }
    }
}

/// `"true"` and `"false"` go back out as JSON booleans; a URL that is literally
/// the string `true` is the one known collision, and is accepted.
struct JsonAttr<'a>(&'a str);

impl Serialize for JsonAttr<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            "true" => serializer.serialize_bool(true),
            "false" => serializer.serialize_bool(false),
            other => serializer.serialize_str(other),
        }
    }
}

/// The requested-attribute wire shape, where `null` means "remove this key".
mod attrs_json {
    use std::collections::BTreeMap;

    use serde::{Deserialize as _, Deserializer, Serializer};

    use super::{Attrs, JsonAttr, RawAttr};

    pub(super) fn serialize<S: Serializer>(
        attrs: &Option<Attrs>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match *attrs {
            None => serializer.serialize_none(),
            Some(ref map) => serializer.collect_map(
                map.iter()
                    .map(|(key, value)| (key, value.as_deref().map(JsonAttr))),
            ),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Attrs>, D::Error> {
        let raw = Option::<BTreeMap<String, Option<RawAttr>>>::deserialize(deserializer)?;
        Ok(raw.map(|map| {
            map.into_iter()
                .map(|(key, value)| (key, value.map(RawAttr::canonical)))
                .collect()
        }))
    }
}

/// The rendered-attribute wire shape, which never carries a null.
mod span_attrs_json {
    use std::collections::BTreeMap;

    use serde::{Deserialize as _, Deserializer, Serializer};

    use super::{JsonAttr, RawAttr};

    pub(super) fn serialize<S: Serializer>(
        attrs: &BTreeMap<String, String>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_map(attrs.iter().map(|(key, value)| (key, JsonAttr(value))))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<String, String>, D::Error> {
        let raw = BTreeMap::<String, RawAttr>::deserialize(deserializer)?;
        Ok(raw
            .into_iter()
            .map(|(key, value)| (key, value.canonical()))
            .collect())
    }
}
