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

use calimero_storage::collections::fugue::{FugueTree, RawId};
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
        if (start..end).all(|index| self.attrs_at(index).get(key).map(String::as_str) == value) {
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
        let mut out: Vec<Span> = Vec::new();
        for (index, content) in self.text().chars().enumerate() {
            let attributes = self.attrs_at(index);
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

    /// Per key, the covering mark with the greatest id wins; `None` removes.
    fn attrs_at(&self, index: usize) -> BTreeMap<String, String> {
        let mut best: BTreeMap<&str, (MarkId, Option<&str>)> = BTreeMap::new();
        for mark in &self.marks {
            let (Some(start), Some(end)) = (self.resolve(&mark.start), self.resolve(&mark.end))
            else {
                continue;
            };
            if index < start || index >= end {
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
