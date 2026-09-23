//! The naive rich-text model the conformance and fuzz suites differentiate
//! against. A directory module, since `tests/rich_text_model.rs` would be
//! compiled as a test target of its own; each binary uses a subset.
//!
//! It shares nothing with the implementation but the pure `FugueTree` and the
//! specification. It resolves an anchor by scanning the document, computes each
//! character's attributes on its own, and run-length-encodes only at the end -
//! so an off-by-one in the implementation's boundary sweep shows up as a
//! disagreement rather than as the same mistake made twice.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use calimero_storage::collections::fugue::{FugueNode, FugueTree, RawId};
use calimero_storage::collections::fugue_text::{Anchor, Bias, IdRange};
use calimero_storage::collections::{Expand, Mark, MarkId, MarkSchema, Span};

/// One rich-text replica, with no storage anywhere.
#[derive(Clone, Debug, Default)]
pub struct Model {
    tree: FugueTree,
    next: BTreeMap<u64, u32>,
    marks: Vec<Mark>,
}

impl Model {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ---- text ----

    /// Insert at visible position `pos`, following Peritext's tombstone rule so
    /// the new characters land on the same side of a formatting boundary the
    /// implementation puts them.
    pub fn insert(&mut self, pos: usize, replica: u64, text: &str) -> Option<IdRange> {
        let mut rest = text.chars();
        let first = rest.next()?;
        let mut order = self.tree.order();
        let start = self.take_counter(replica);
        let id = (replica, start);

        match self.boundary_left(pos) {
            Some(left) => {
                let _node = self
                    .tree
                    .insert_after_in(&mut order, Some(left), first, id)
                    .expect("the boundary tombstone is attached");
            }
            None => {
                let _node = self
                    .tree
                    .insert_in(&mut order, pos, first, id)
                    .expect("caller keeps the position inside the document");
            }
        }

        let mut previous = id;
        let mut len = 1_u32;
        for content in rest {
            let next = (replica, self.take_counter(replica));
            let _node = self
                .tree
                .insert_after_in(&mut order, Some(previous), content, next)
                .expect("the previous character is attached");
            previous = next;
            len += 1;
        }
        Some(IdRange { start: id, len })
    }

    pub fn delete(&mut self, start: usize, end: usize) {
        let count = end.min(self.len()).saturating_sub(start);
        for _ in 0..count {
            if self.tree.delete(start).is_err() {
                break;
            }
        }
    }

    pub fn delete_ids(&mut self, ids: &IdRange) {
        let order = self.tree.order();
        let end = u64::from(ids.start.1) + u64::from(ids.len);
        let _picked = self.tree.delete_ids_in(&order, |(replica, counter)| {
            replica == ids.start.0 && counter >= ids.start.1 && u64::from(counter) < end
        });
    }

    #[must_use]
    pub fn text(&self) -> String {
        self.tree.values()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tree.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // ---- marks ----

    /// Returns the id written, or `None` when the range was empty or the write
    /// was redundant. Errors are the caller's to avoid: the model asserts the
    /// specification rather than reimplementing its validation.
    pub fn mark<Sc: MarkSchema>(
        &mut self,
        start: usize,
        end: usize,
        key: &str,
        value: Option<&str>,
        replica: u64,
    ) -> Option<MarkId> {
        let prefix = key.split(':').next().unwrap_or(key);
        let mut expand = Sc::expand(prefix).expect("the model is driven with declared keys");
        if value.is_none() {
            expand = expand.inverted();
        }
        if start >= end || end > self.len() {
            return None;
        }
        let resolved = self.resolved();
        if (start..end)
            .all(|index| attrs_from(&resolved, index).get(key).map(String::as_str) == value)
        {
            return None;
        }

        let id = MarkId {
            lamport: self.marks.iter().map(|m| m.id.lamport).max().unwrap_or(0) + 1,
            replica,
        };
        self.marks.push(Mark {
            id,
            start: self.anchor_at(start, edge_bias(expand, true)),
            end: self.anchor_at(end, edge_bias(expand, false)),
            key: key.to_owned(),
            value: value.map(str::to_owned),
        });
        Some(id)
    }

    pub fn mark_rows(&self) -> Vec<Mark> {
        let mut marks = self.marks.clone();
        marks.sort_by_key(|mark| mark.id);
        marks
    }

    /// The document as attributed runs, built one character at a time.
    #[must_use]
    pub fn spans(&self) -> Vec<Span> {
        let resolved = self.resolved();
        let mut out: Vec<Span> = Vec::new();
        for (index, content) in self.text().chars().enumerate() {
            let attributes = attrs_from(&resolved, index);
            match out.last_mut() {
                Some(last) if last.attributes == attributes => last.text.push(content),
                _ => out.push(Span {
                    text: content.to_string(),
                    attributes,
                }),
            }
        }
        out
    }

    #[must_use]
    pub fn attributes_at(&self, index: usize) -> BTreeMap<String, String> {
        attrs_from(&self.resolved(), index)
    }

    /// Each mark placed by its own linear scan. One scan per mark, not one per
    /// (character, mark) pair, which is the difference between naive and unusable.
    fn resolved(&self) -> Vec<(usize, usize, &Mark)> {
        self.marks
            .iter()
            .filter_map(|mark| {
                let start = self.resolve(&mark.start)?;
                let end = self.resolve(&mark.end)?;
                Some((start, end, mark))
            })
            .collect()
    }

    // ---- anchors, by linear scan ----

    #[must_use]
    pub fn resolve(&self, anchor: &Anchor) -> Option<usize> {
        let (wanted, bias) = match *anchor {
            Anchor::Start => return Some(0),
            Anchor::End => return Some(self.len()),
            Anchor::Char { id, bias } => (id, bias),
        };
        let mut live = 0_usize;
        for id in self.tree.order().into_iter().flatten() {
            let Some(node) = self.tree.node(id) else {
                continue;
            };
            if id == wanted {
                return Some(live + usize::from(node.value.is_some() && bias == Bias::After));
            }
            live += usize::from(node.value.is_some());
        }
        None
    }

    #[must_use]
    pub fn anchor_at(&self, pos: usize, bias: Bias) -> Anchor {
        let len = self.len();
        let index = match bias {
            Bias::After if pos == 0 => return Anchor::Start,
            Bias::Before if pos == len => return Anchor::End,
            Bias::After => pos - 1,
            Bias::Before => pos,
        };
        Anchor::Char {
            id: self
                .live_id(index)
                .expect("position is inside the document"),
            bias,
        }
    }

    fn live_id(&self, index: usize) -> Option<RawId> {
        self.tree
            .order()
            .into_iter()
            .flatten()
            .filter(|id| self.tree.node(*id).is_some_and(|n| n.value.is_some()))
            .nth(index)
    }

    /// The last tombstone in the gap at `pos` that carries an `After` anchor.
    fn boundary_left(&self, pos: usize) -> Option<RawId> {
        let anchored: BTreeSet<RawId> = self
            .marks
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
        for id in self.tree.order().into_iter().flatten() {
            let Some(node) = self.tree.node(id) else {
                continue;
            };
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

    // ---- deltas, for driving several replicas ----

    /// The nodes and mark rows an edit added or tombstoned, which is what one
    /// delta carries. Mirrors what the storage layer puts on the wire.
    #[must_use]
    pub fn diff(before: &Self, after: &Self) -> ModelDelta {
        let old: BTreeMap<RawId, FugueNode> = before.nodes();
        let held: BTreeSet<MarkId> = before.marks.iter().map(|mark| mark.id).collect();
        ModelDelta {
            nodes: after
                .nodes()
                .into_iter()
                .filter(|(id, node)| old.get(id).is_none_or(|was| was.value != node.value))
                .map(|(_, node)| node)
                .collect(),
            marks: after
                .marks
                .iter()
                .filter(|mark| !held.contains(&mark.id))
                .cloned()
                .collect(),
        }
    }

    pub fn integrate(&mut self, delta: &ModelDelta) {
        for node in &delta.nodes {
            self.tree.integrate(*node);
        }
        // Delete wins, so a tombstone that arrived before its node re-applies.
        for node in &delta.nodes {
            if node.value.is_none() {
                self.tree.integrate(*node);
            }
        }
        let held: BTreeSet<MarkId> = self.marks.iter().map(|mark| mark.id).collect();
        for mark in &delta.marks {
            if !held.contains(&mark.id) {
                self.marks.push(mark.clone());
            }
        }
    }

    fn nodes(&self) -> BTreeMap<RawId, FugueNode> {
        self.tree.nodes().map(|node| (node.id, *node)).collect()
    }

    // ---- what a coverage guard needs to see ----

    /// Every mark that resolves here, as `(start, end, key)`.
    #[must_use]
    pub fn active_ranges(&self) -> Vec<(usize, usize, String)> {
        self.resolved()
            .into_iter()
            .filter(|(start, end, _)| start < end)
            .map(|(start, end, mark)| (start, end, mark.key.clone()))
            .collect()
    }

    /// Marks this replica stores but cannot place, which a read must skip.
    #[must_use]
    pub fn inactive_marks(&self) -> usize {
        self.marks.len() - self.active_ranges().len()
    }

    /// Whether the gap at `pos` holds at least one tombstone.
    #[must_use]
    pub fn has_tombstones_at(&self, pos: usize) -> bool {
        let mut live = 0_usize;
        for id in self.tree.order().into_iter().flatten() {
            let Some(node) = self.tree.node(id) else {
                continue;
            };
            if node.value.is_some() {
                if live == pos {
                    return false;
                }
                live += 1;
            } else if live == pos {
                return true;
            }
        }
        false
    }

    /// The biases of every active mark edge sitting exactly at `pos`: `true`
    /// where the edge grows over an insert there, `false` where it does not.
    #[must_use]
    pub fn edges_at(&self, pos: usize) -> Vec<bool> {
        let mut out = Vec::new();
        for mark in &self.marks {
            for (anchor, is_end) in [(mark.start, false), (mark.end, true)] {
                let Some(at) = self.resolve(&anchor) else {
                    continue;
                };
                if at != pos {
                    continue;
                }
                let grows = match anchor {
                    Anchor::Start | Anchor::End => true,
                    Anchor::Char { bias, .. } => {
                        if is_end {
                            bias == Bias::Before
                        } else {
                            bias == Bias::After
                        }
                    }
                };
                out.push(grows);
            }
        }
        out
    }

    // ---- merge ----

    /// The state-based join: every node, delete-wins, plus the union of the mark
    /// rows by id. Write-once makes the mark half a set union and nothing more.
    pub fn merge(&mut self, other: &Self) {
        let nodes: Vec<_> = other.tree.nodes().copied().collect();
        for node in &nodes {
            self.tree.integrate(*node);
        }
        for node in &nodes {
            if node.value.is_none() {
                self.tree.integrate(*node);
            }
        }
        let held: BTreeSet<MarkId> = self.marks.iter().map(|mark| mark.id).collect();
        for mark in &other.marks {
            if !held.contains(&mark.id) {
                self.marks.push(mark.clone());
            }
        }
        for (replica, counter) in &other.next {
            let slot = self.next.entry(*replica).or_default();
            *slot = (*slot).max(*counter);
        }
    }

    fn take_counter(&mut self, replica: u64) -> u32 {
        let slot = self.next.entry(replica).or_default();
        let counter = *slot;
        *slot += 1;
        counter
    }
}

/// Per key, the covering mark with the greatest id wins; `None` removes the key.
fn attrs_from(resolved: &[(usize, usize, &Mark)], index: usize) -> BTreeMap<String, String> {
    let mut best: BTreeMap<&str, (MarkId, Option<&str>)> = BTreeMap::new();
    for (start, end, mark) in resolved {
        if index < *start || index >= *end {
            continue;
        }
        let winner = best
            .entry(mark.key.as_str())
            .or_insert((mark.id, mark.value.as_deref()));
        if mark.id > winner.0 {
            *winner = (mark.id, mark.value.as_deref());
        }
    }
    best.into_iter()
        .filter_map(|(key, (_, value))| value.map(|value| (key.to_owned(), value.to_owned())))
        .collect()
}

/// What one edit added or tombstoned.
#[derive(Clone, Debug, Default)]
pub struct ModelDelta {
    pub nodes: Vec<FugueNode>,
    pub marks: Vec<Mark>,
}

/// A growing edge is anchored across the gap so a later insert lands inside it;
/// a non-growing edge hugs the range's own character so the insert lands outside.
const fn edge_bias(expand: Expand, is_start: bool) -> Bias {
    let grows = if is_start {
        matches!(expand, Expand::Before | Expand::Both)
    } else {
        matches!(expand, Expand::After | Expand::Both)
    };
    match (is_start, grows) {
        (true, true) | (false, false) => Bias::After,
        (true, false) | (false, true) => Bias::Before,
    }
}

/// A span list as `(text, sorted attribute pairs)`, which is what an assertion
/// failure should print rather than a wall of `BTreeMap` debug output.
#[must_use]
pub fn readable(spans: &[Span]) -> Vec<(String, Vec<(String, String)>)> {
    spans
        .iter()
        .map(|span| {
            (
                span.text.clone(),
                span.attributes
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        })
        .collect()
}

pub const NONE: &[(&str, &str)] = &[];
pub const BOLD: &[(&str, &str)] = &[("bold", "true")];

/// `(text, attribute pairs)` per span, which is what an expectation reads as.
pub fn expect(spans: &[Span], want: &[(&str, &[(&str, &str)])]) {
    let want: Vec<(String, Vec<(String, String)>)> = want
        .iter()
        .map(|(text, pairs)| {
            (
                (*text).to_owned(),
                pairs
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(readable(spans), want);
}

/// One block of the document model: the structure fields, each stamped with the
/// write that set it, plus a [`Model`] body.
///
/// The stamp is what lets the model resolve a field two replicas wrote at
/// different times the way the storage layer's last-write-wins does. It is NOT a
/// clock: the fuzz that drives this applies operations in one sequence, so a
/// later write always carries a greater stamp, and the generator never lets two
/// replicas write one field concurrently - a tie no model can predict.
#[derive(Clone, Debug)]
pub struct DocBlock {
    pub id: RawId,
    pub kind: (u64, String),
    pub depth: (u64, u8),
    pub place: (u64, Anchor),
    pub deleted: (u64, bool),
    pub attrs: BTreeMap<String, (u64, Option<String>)>,
    pub body: Model,
}

/// What a rendered block is compared as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocView {
    pub id: RawId,
    pub kind: String,
    pub depth: u8,
    pub attrs: BTreeMap<String, String>,
    pub spans: Vec<Span>,
}

/// The naive document model: a spine `FugueTree` of placeholder characters over
/// block ids, plus one row per block. It shares nothing with the implementation
/// but the pure tree and the specification.
#[derive(Clone, Debug, Default)]
pub struct DocModel {
    spine: FugueTree,
    next: BTreeMap<u64, u32>,
    blocks: Vec<DocBlock>,
}

impl DocModel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ---- structure ----

    /// Mint a spine slot directly after `after` and open a block on it.
    pub fn insert_block(
        &mut self,
        after: Option<RawId>,
        kind: &str,
        depth: u8,
        replica: u64,
        stamp: u64,
    ) -> RawId {
        let at = self.slot_for(after);
        let counter = self.take_counter(replica);
        let id = (replica, counter);
        let mut order = self.spine.order();
        let _node = self
            .spine
            .insert_in(&mut order, at, SPINE_SLOT, id)
            .expect("the slot position is inside the spine");

        self.blocks.push(DocBlock {
            id,
            kind: (stamp, kind.to_owned()),
            depth: (stamp, depth),
            place: (
                0,
                Anchor::Char {
                    id,
                    bias: Bias::Before,
                },
            ),
            deleted: (0, false),
            attrs: BTreeMap::new(),
            body: Model::new(),
        });
        id
    }

    pub fn move_block(&mut self, block: RawId, after: Option<RawId>, replica: u64, stamp: u64) {
        let at = self.slot_for(after);
        let counter = self.take_counter(replica);
        let slot = (replica, counter);
        let mut order = self.spine.order();
        let _node = self
            .spine
            .insert_in(&mut order, at, SPINE_SLOT, slot)
            .expect("the slot position is inside the spine");
        self.block_mut(block).place = (
            stamp,
            Anchor::Char {
                id: slot,
                bias: Bias::Before,
            },
        );
    }

    pub fn delete_block(&mut self, block: RawId, stamp: u64) {
        self.block_mut(block).deleted = (stamp, true);
    }

    pub fn set_kind(&mut self, block: RawId, kind: &str, stamp: u64) {
        self.block_mut(block).kind = (stamp, kind.to_owned());
    }

    pub fn set_depth(&mut self, block: RawId, depth: u8, stamp: u64) {
        self.block_mut(block).depth = (stamp, depth);
    }

    pub fn set_attr(&mut self, block: RawId, key: &str, value: Option<&str>, stamp: u64) {
        let _old = self
            .block_mut(block)
            .attrs
            .insert(key.to_owned(), (stamp, value.map(str::to_owned)));
    }

    /// The tail's text AND its rendered attributes move into a new block; the
    /// original keeps only what precedes `at`.
    pub fn split_block<Sc: MarkSchema>(
        &mut self,
        block: RawId,
        at: usize,
        replica: u64,
        stamp: u64,
    ) -> RawId {
        let row = self.block(block).expect("the split target exists");
        let len = row.body.len();
        let tail = slice_spans(&row.body.spans(), at, len);
        let (kind, depth) = (row.kind.1.clone(), row.depth.1);
        let attrs: Vec<(String, Option<String>)> = row
            .attrs
            .iter()
            .filter_map(|(key, (_, value))| value.clone().map(|value| (key.clone(), Some(value))))
            .collect();

        let new = self.insert_block(Some(block), &kind, depth, replica, stamp);
        for (key, value) in attrs {
            self.set_attr(new, &key, value.as_deref(), stamp);
        }
        if at < len {
            carry::<Sc>(&mut self.block_mut(new).body, 0, &tail, replica);
            self.block_mut(block).body.delete(at, len);
        }
        new
    }

    pub fn merge_blocks<Sc: MarkSchema>(
        &mut self,
        first: RawId,
        second: RawId,
        replica: u64,
        stamp: u64,
    ) {
        let tail = self.block(second).expect("the source exists").body.spans();
        let at = self.block(first).expect("the target exists").body.len();
        if !tail.is_empty() {
            carry::<Sc>(&mut self.block_mut(first).body, at, &tail, replica);
        }
        self.delete_block(second, stamp);
    }

    // ---- reads ----

    /// The ordered, non-deleted blocks. A block whose spine slot this replica
    /// cannot place renders at the END, by id.
    #[must_use]
    pub fn views(&self) -> Vec<DocView> {
        let mut live: Vec<(Placed, &DocBlock)> = self
            .blocks
            .iter()
            .filter(|row| !row.deleted.1)
            .map(|row| {
                let at = self.resolve(&row.place.1);
                ((at.is_none(), at, row.id), row)
            })
            .collect();
        live.sort_by_key(|(key, _)| *key);
        live.into_iter()
            .map(|(_, row)| DocView {
                id: row.id,
                kind: row.kind.1.clone(),
                depth: row.depth.1,
                attrs: row
                    .attrs
                    .iter()
                    .filter_map(|(key, (_, value))| value.clone().map(|value| (key.clone(), value)))
                    .collect(),
                spans: row.body.spans(),
            })
            .collect()
    }

    #[must_use]
    pub fn live_ids(&self) -> Vec<RawId> {
        self.views().into_iter().map(|view| view.id).collect()
    }

    #[must_use]
    pub fn block(&self, id: RawId) -> Option<&DocBlock> {
        self.blocks.iter().find(|row| row.id == id)
    }

    pub fn block_mut(&mut self, id: RawId) -> &mut DocBlock {
        self.blocks
            .iter_mut()
            .find(|row| row.id == id)
            .expect("the block exists")
    }

    // ---- merge ----

    /// The state-based join: the spine by delete-wins integration, each block
    /// field by the greater stamp, each body by [`Model::merge`].
    pub fn merge(&mut self, other: &Self) {
        let nodes: Vec<_> = other.spine.nodes().copied().collect();
        for node in &nodes {
            self.spine.integrate(*node);
        }
        for node in nodes.iter().filter(|node| node.value.is_none()) {
            self.spine.integrate(*node);
        }
        for (replica, counter) in &other.next {
            let slot = self.next.entry(*replica).or_default();
            *slot = (*slot).max(*counter);
        }

        for incoming in &other.blocks {
            let Some(mine) = self.blocks.iter_mut().find(|row| row.id == incoming.id) else {
                self.blocks.push(incoming.clone());
                continue;
            };
            newer(&mut mine.kind, &incoming.kind);
            newer(&mut mine.depth, &incoming.depth);
            newer(&mut mine.place, &incoming.place);
            newer(&mut mine.deleted, &incoming.deleted);
            for (key, value) in &incoming.attrs {
                match mine.attrs.get_mut(key) {
                    Some(held) => newer(held, value),
                    None => {
                        let _new = mine.attrs.insert(key.clone(), value.clone());
                    }
                }
            }
            mine.body.merge(&incoming.body);
        }
    }

    // ---- internals ----

    /// Where a slot placed after `after` goes. A block whose own slot has not
    /// arrived renders at the end, so a block placed after it goes there too.
    fn slot_for(&self, after: Option<RawId>) -> usize {
        let Some(after) = after else { return 0 };
        let place = self.block(after).map_or(Anchor::End, |row| row.place.1);
        self.resolve(&place)
            .map_or_else(|| self.spine.len(), |at| at + 1)
    }

    fn resolve(&self, anchor: &Anchor) -> Option<usize> {
        let (wanted, bias) = match *anchor {
            Anchor::Start => return Some(0),
            Anchor::End => return Some(self.spine.len()),
            Anchor::Char { id, bias } => (id, bias),
        };
        let mut live = 0_usize;
        for id in self.spine.order().into_iter().flatten() {
            let Some(node) = self.spine.node(id) else {
                continue;
            };
            if id == wanted {
                return Some(live + usize::from(node.value.is_some() && bias == Bias::After));
            }
            live += usize::from(node.value.is_some());
        }
        None
    }

    fn take_counter(&mut self, replica: u64) -> u32 {
        let slot = self.next.entry(replica).or_default();
        let counter = *slot;
        *slot += 1;
        counter
    }
}

/// One placeholder per block, matching the implementation's spine character.
const SPINE_SLOT: char = '\u{FFFC}';

/// A block's read order: unplaced last, then spine position, then id.
type Placed = (bool, Option<usize>, RawId);

fn newer<T: Clone>(mine: &mut (u64, T), incoming: &(u64, T)) {
    if incoming.0 > mine.0 {
        *mine = incoming.clone();
    }
}

/// The part of `spans` covering visible positions `start..end`.
#[must_use]
pub fn slice_spans(spans: &[Span], start: usize, end: usize) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = 0_usize;
    for span in spans {
        let len = span.text.chars().count();
        let (from, to) = (at.max(start), (at + len).min(end));
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

/// Insert `spans` at `at`, re-asserting each span's attributes as the COMPLETE
/// desired set: the model's mirror of carrying a tail into another body, where a
/// key the previous span left inherited is stripped rather than kept.
pub fn carry<Sc: MarkSchema>(body: &mut Model, at: usize, spans: &[Span], replica: u64) {
    let mut pos = at;
    for span in spans {
        let count = span.text.chars().count();
        if count == 0 {
            continue;
        }
        let _minted = body.insert(pos, replica, &span.text);
        let (start, end) = (pos, pos + count);
        let inherited = body.attributes_at(start);
        let keys: BTreeSet<&String> = span.attributes.keys().chain(inherited.keys()).collect();
        for key in keys {
            let want = span.attributes.get(key).map(String::as_str);
            let _id = body.mark::<Sc>(start, end, key, want, replica);
        }
        pos = end;
    }
}
