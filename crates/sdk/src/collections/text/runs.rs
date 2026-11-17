//! The run layout: runs of characters, ordered by a counted B+ tree.
//!
//! - `(name, "r", start)` is a run `{text, dead}`: the characters with ids `start`,
//!   `start + 1`, .. that follow each other in the document; `text` holds the live ones and
//!   `dead` the deleted offsets as sorted `[from, to)` pairs. A run holds at most
//!   [`MAX_RUN_CHARS`] characters and is split only where an insert lands inside it.
//! - `(name, "h")` is the root of the tree and `(name, "x", id)` every other node
//!   `{leaf, parent, items}`: `items` are `[key, live]` pairs in document order, `key` a
//!   run's start in a leaf or a child node's id above, `live` the live characters under it;
//!   `parent` is the parent's id, null for the root's children. Nodes hold at most
//!   [`FANOUT`] items and only ever split (runs are never removed), so every leaf is at the
//!   same depth.
//! - `(name, "l", start)` is the leaf holding a run's item: a node id, null for the root.
//!
//! An edit reads and writes the run it touches, its leaf and the leaf's ancestors, so it
//! costs O(log runs) host calls and bytes. Every row an edit reads is cached in a [`Doc`],
//! changed there, and written once by [`Doc::plan`].

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{key, Done, Edit, Plan, LEN, NEXT};
use crate::commons::{bounds, key_after, read_count, KeyPath};
use crate::encoding::key::{self as tuple, KeyPart};
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{RangeCursor, Store, MAX_RANGE_BATCH};

/// Where a run is stored.
pub(super) const RUN: &str = "r";
/// A run's leaf.
pub(super) const LINK: &str = "l";
/// A tree node other than the root.
pub(super) const NODE: &str = "x";
/// The root node.
pub(super) const ROOT: &str = "h";

/// The most characters one run holds: a 10,000-character paste is five runs.
const MAX_RUN_CHARS: usize = 2048;
/// Typing right after a run's last character grows the run while it is shorter than this,
/// so a keystroke rewrites at most this many characters.
const MAX_GROWN_CHARS: u64 = 512;
/// The most items a tree node holds.
const FANOUT: usize = 64;
/// The deepest tree a stored document can have (64^8 runs); past it the rows are corrupt.
const MAX_DEPTH: usize = 8;

/// A reference to a node: `None` is the root.
type NodeRef = Option<u64>;

/// The characters of one run, `None` where deleted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Run {
    pub(crate) cells: Vec<Option<char>>,
}

/// A run as stored.
#[derive(Serialize, Deserialize)]
struct RunRow {
    text: String,
    dead: Vec<(u32, u32)>,
}

impl Run {
    pub(crate) fn len(&self) -> u64 {
        self.cells.len() as u64
    }

    pub(crate) fn live(&self) -> u64 {
        self.cells.iter().filter(|c| c.is_some()).count() as u64
    }

    /// Live characters before offset `off`.
    fn live_before(&self, off: u64) -> u64 {
        let off = usize::try_from(off).unwrap_or(usize::MAX);
        self.cells.iter().take(off).filter(|c| c.is_some()).count() as u64
    }

    /// The offset of the `k`th live character.
    fn nth_live(&self, k: u64) -> Option<u64> {
        let k = usize::try_from(k).ok()?;
        self.cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_some())
            .nth(k)
            .map(|(off, _)| off as u64)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, StorageError> {
        let mut row = RunRow {
            text: String::new(),
            dead: Vec::new(),
        };
        for (off, cell) in self.cells.iter().enumerate() {
            let off =
                u32::try_from(off).map_err(|_| StorageError::Encode("run too long".into()))?;
            match (cell, row.dead.last_mut()) {
                (Some(ch), _) => row.text.push(*ch),
                (None, Some((_, to))) if *to == off => *to = off + 1,
                (None, _) => row.dead.push((off, off + 1)),
            }
        }
        value::to_vec(&row)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, StorageError> {
        let row: RunRow = value::from_slice(bytes)?;
        let bad = || StorageError::Decode("malformed text run".into());
        let mut text = row.text.chars();
        let mut cells = Vec::new();
        for (from, to) in row.dead {
            let (from, to) = (from as usize, to as usize);
            if from < cells.len() || to <= from || to > MAX_RUN_CHARS {
                return Err(bad());
            }
            while cells.len() < from {
                cells.push(Some(text.next().ok_or_else(bad)?));
            }
            cells.resize(to, None);
        }
        cells.extend(text.map(Some));
        if cells.is_empty() || cells.len() > MAX_RUN_CHARS {
            return Err(bad());
        }
        Ok(Self { cells })
    }
}

/// A tree node as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Node {
    leaf: bool,
    parent: NodeRef,
    items: Vec<(u64, u64)>,
}

impl Node {
    fn empty_root() -> Self {
        Self {
            leaf: true,
            parent: None,
            items: Vec::new(),
        }
    }

    fn position(&self, key: u64) -> Result<usize, StorageError> {
        self.items
            .iter()
            .position(|(k, _)| *k == key)
            .ok_or_else(|| StorageError::Decode("text tree out of step".into()))
    }

    fn live(&self) -> u64 {
        self.items.iter().map(|(_, live)| live).sum()
    }
}

fn shift(live: u64, delta: i64) -> Result<u64, StorageError> {
    live.checked_add_signed(delta)
        .ok_or_else(|| StorageError::Decode("text tree count out of step".into()))
}

/// The rows one edit reads and changes, cached: see the module docs.
pub(super) struct Doc<'a, S: Store> {
    path: &'a KeyPath,
    store: &'a S,
    runs: BTreeMap<u64, Run>,
    dirty_runs: BTreeSet<u64>,
    links: BTreeMap<u64, NodeRef>,
    dirty_links: BTreeSet<u64>,
    nodes: BTreeMap<NodeRef, Node>,
    dirty_nodes: BTreeSet<NodeRef>,
    next_id: Option<u64>,
    minted: u64,
    live_delta: i64,
}

impl<'a, S: Store> Doc<'a, S> {
    pub(super) fn open(path: &'a KeyPath, store: &'a S) -> Self {
        Self {
            path,
            store,
            runs: BTreeMap::new(),
            dirty_runs: BTreeSet::new(),
            links: BTreeMap::new(),
            dirty_links: BTreeSet::new(),
            nodes: BTreeMap::new(),
            dirty_nodes: BTreeSet::new(),
            next_id: None,
            minted: 0,
            live_delta: 0,
        }
    }

    /// Apply an edit to the cached rows.
    pub(super) fn edit(&mut self, edit: &Edit<'_>) -> Result<Done, StorageError> {
        Ok(match *edit {
            Edit::InsertAfter(after, text) => Done::Ids(self.insert_after(after, text)?),
            Edit::Insert(index, text) => {
                let after = match index.checked_sub(1) {
                    None => None,
                    Some(i) => match self.id_at(i)? {
                        Some(id) => Some(id),
                        None => {
                            let len = read_count(
                                self.store,
                                &key(self.path, LEN, None)?,
                                self.path.label(),
                            )?;
                            return Err(StorageError::OutOfBounds { index, len });
                        }
                    },
                };
                Done::Ids(self.insert_after(after, text)?)
            }
            Edit::DeleteIds(ids) => Done::Count(self.delete_ids(ids)?),
            Edit::Delete(index, count) => Done::Count(self.delete(index, count)?),
        })
    }

    /// The writes the cached changes need.
    pub(super) fn plan(self) -> Result<Plan, StorageError> {
        let mut plan = Plan::default();
        for start in &self.dirty_runs {
            let run = self.runs.get(start).ok_or_else(missing)?;
            plan.put(key(self.path, RUN, Some(*start))?, run.encode()?);
        }
        for start in &self.dirty_links {
            let leaf = self.links.get(start).ok_or_else(missing)?;
            plan.put(key(self.path, LINK, Some(*start))?, value::to_vec(leaf)?);
        }
        for r in &self.dirty_nodes {
            let node = self.nodes.get(r).ok_or_else(missing)?;
            plan.put(node_key(self.path, *r)?, value::to_vec(node)?);
        }
        if self.minted > 0 {
            plan.add(key(self.path, NEXT, None)?, self.minted)?;
        }
        match u64::try_from(self.live_delta) {
            Ok(0) => {}
            Ok(n) => plan.add(key(self.path, LEN, None)?, n)?,
            Err(_) => plan.sub(key(self.path, LEN, None)?, self.live_delta.unsigned_abs())?,
        }
        Ok(plan)
    }

    // --- reads -----------------------------------------------------------------------

    pub(super) fn id_at(&mut self, index: u64) -> Result<Option<u64>, StorageError> {
        let Some((leaf, i, k)) = self.seek(index)? else {
            return Ok(None);
        };
        let start = self.node(leaf)?.items[i].0;
        let off = self.run(start)?.nth_live(k).ok_or_else(out_of_step)?;
        Ok(Some(start + off))
    }

    pub(super) fn index_of(&mut self, id: u64) -> Result<Option<u64>, StorageError> {
        let Some(start) = self.find_run(id)? else {
            return Ok(None);
        };
        let within = self.run(start)?.live_before(id - start);
        let leaf = self.leaf_of(start)?;
        let i = self.node(leaf)?.position(start)?;
        let mut before = within + self.node(leaf)?.items[..i].iter().map(|x| x.1).sum::<u64>();
        let mut cur = leaf;
        while let Some(id) = cur {
            let parent = self.node(cur)?.parent;
            let p = self.node(parent)?;
            let j = p.position(id)?;
            before += p.items[..j].iter().map(|x| x.1).sum::<u64>();
            cur = parent;
        }
        Ok(Some(before))
    }

    /// Where `id` stands in the document, tombstones included, as a key that sorts in
    /// document order: the item positions from the root down, then the offset in its run.
    fn place(&mut self, id: u64) -> Result<Vec<u64>, StorageError> {
        let start = self.find_run(id)?.ok_or(StorageError::UnknownId(id))?;
        let leaf = self.leaf_of(start)?;
        let mut place = vec![id - start, self.node(leaf)?.position(start)? as u64];
        let mut cur = leaf;
        while let Some(id) = cur {
            let parent = self.node(cur)?.parent;
            place.push(self.node(parent)?.position(id)? as u64);
            cur = parent;
        }
        place.reverse();
        Ok(place)
    }

    /// `first` and `last` in document order (either may come first) and the id after the
    /// later one; `UnknownId` when either never existed.
    pub(super) fn ordered(
        &mut self,
        first: u64,
        last: u64,
    ) -> Result<(u64, u64, Option<u64>), StorageError> {
        let (a, b) = if self.place(first)? <= self.place(last)? {
            (first, last)
        } else {
            (last, first)
        };
        let start = self.find_run(b)?.ok_or(StorageError::UnknownId(b))?;
        if b + 1 < start + self.run(start)?.len() {
            return Ok((a, b, Some(b + 1)));
        }
        let leaf = self.leaf_of(start)?;
        let i = self.node(leaf)?.position(start)?;
        let next = match self.node(leaf)?.items.get(i + 1) {
            Some((s, _)) => Some(*s),
            None => match self.next_leaf(leaf)? {
                Some(next) => self.node(next)?.items.first().map(|(s, _)| *s),
                None => None,
            },
        };
        Ok((a, b, next))
    }

    // --- edits -----------------------------------------------------------------------

    fn insert_after(&mut self, after: Option<u64>, text: &str) -> Result<Vec<u64>, StorageError> {
        let target = match after {
            None => None,
            Some(x) => {
                let start = self.find_run(x)?.ok_or(StorageError::UnknownId(x))?;
                Some((start, x - start))
            }
        };
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            return Ok(Vec::new());
        }
        let count = chars.len() as u64;
        let first = self.mint(count)?;
        self.live_delta = self
            .live_delta
            .checked_add_unsigned(count)
            .ok_or_else(|| StorageError::Encode("text too long".into()))?;
        let ids = (first..first + count).collect();

        // Typing right after a run's last character with the next ids: the run grows.
        if let Some((start, off)) = target {
            let run = self.run(start)?;
            let len = run.len();
            if off + 1 == len && first == start + len && len + count <= MAX_GROWN_CHARS {
                run.cells.extend(chars.into_iter().map(Some));
                let _new = self.dirty_runs.insert(start);
                let leaf = self.leaf_of(start)?;
                self.shift_item(leaf, start, i64::try_from(count).map_err(too_long)?)?;
                return Ok(ids);
            }
        }

        let mut items = Vec::new();
        for (n, chunk) in chars.chunks(MAX_RUN_CHARS).enumerate() {
            let start = first + (n * MAX_RUN_CHARS) as u64;
            let _new = self.dirty_runs.insert(start);
            let _old = self.runs.insert(
                start,
                Run {
                    cells: chunk.iter().copied().map(Some).collect(),
                },
            );
            items.push((start, chunk.len() as u64));
        }
        let (leaf, at) = match target {
            None => (self.first_leaf()?, 0),
            Some((start, off)) => {
                let leaf = self.leaf_of(start)?;
                let at = self.node(leaf)?.position(start)? + 1;
                if off + 1 < self.run(start)?.len() {
                    // The insert lands inside the run: split it after `off`.
                    let tail = Run {
                        cells: self.run(start)?.cells.split_off((off + 1) as usize),
                    };
                    let tail_live = tail.live();
                    let _new = self.dirty_runs.insert(start);
                    let _new = self.dirty_runs.insert(start + off + 1);
                    let _old = self.runs.insert(start + off + 1, tail);
                    self.shift_item(leaf, start, -i64::try_from(tail_live).map_err(too_long)?)?;
                    items.push((start + off + 1, tail_live));
                }
                (leaf, at)
            }
        };
        self.insert_items(leaf, at, items)?;
        Ok(ids)
    }

    fn delete_ids(&mut self, ids: &[u64]) -> Result<u64, StorageError> {
        let mut by_run: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
        for id in ids {
            let start = self.find_run(*id)?.ok_or(StorageError::UnknownId(*id))?;
            let _new = by_run.entry(start).or_default().insert(id - start);
        }
        let mut killed = 0;
        for (start, offs) in by_run {
            let run = self.run(start)?;
            let mut n = 0;
            for off in offs {
                if run.cells[off as usize].take().is_some() {
                    n += 1;
                }
            }
            if n > 0 {
                let _new = self.dirty_runs.insert(start);
                let leaf = self.leaf_of(start)?;
                self.shift_item(leaf, start, -n)?;
            }
            killed += n.unsigned_abs();
        }
        self.live_delta -= i64::try_from(killed).map_err(too_long)?;
        Ok(killed)
    }

    fn delete(&mut self, index: u64, count: u64) -> Result<u64, StorageError> {
        if count == 0 {
            return Ok(0);
        }
        let Some((mut leaf, mut i, mut k)) = self.seek(index)? else {
            return Ok(0);
        };
        let mut left = count;
        let mut leaf_delta: i64 = 0;
        loop {
            let (start, live) = self.node(leaf)?.items[i];
            if live > k {
                let run = self.run(start)?;
                let mut n = 0;
                let mut seen = 0;
                for cell in run.cells.iter_mut() {
                    if cell.is_none() {
                        continue;
                    }
                    if seen >= k && n < left {
                        *cell = None;
                        n += 1;
                    }
                    seen += 1;
                }
                let _new = self.dirty_runs.insert(start);
                self.node_mut(leaf)?.items[i].1 = live - n;
                leaf_delta -= i64::try_from(n).map_err(too_long)?;
                left -= n;
            }
            k = 0;
            if left == 0 {
                break;
            }
            i += 1;
            if i < self.node(leaf)?.items.len() {
                continue;
            }
            self.bump(leaf, leaf_delta)?;
            leaf_delta = 0;
            match self.next_leaf(leaf)? {
                Some(next) => {
                    leaf = next;
                    i = 0;
                }
                None => break,
            }
        }
        self.bump(leaf, leaf_delta)?;
        let done = count - left;
        self.live_delta -= i64::try_from(done).map_err(too_long)?;
        Ok(done)
    }

    // --- the tree --------------------------------------------------------------------

    fn mint(&mut self, n: u64) -> Result<u64, StorageError> {
        let next = match self.next_id {
            Some(next) => next,
            None => {
                let next = read_count(self.store, &key(self.path, NEXT, None)?, self.path.label())?;
                self.next_id = Some(next);
                next
            }
        };
        let first = next + self.minted;
        self.minted += n;
        Ok(first)
    }

    /// The start of the run holding `id`, if any: the cached run, else the store's run with
    /// the greatest start at or before `id` (one reverse cursor read).
    fn find_run(&mut self, id: u64) -> Result<Option<u64>, StorageError> {
        if let Some((start, run)) = self.runs.range(..=id).next_back() {
            if id - start < run.len() {
                return Ok(Some(*start));
            }
        }
        let base = key(self.path, RUN, None)?;
        let mut parts = self.path.parts().to_vec();
        parts.push(KeyPart::Str(RUN.into()));
        parts.push(KeyPart::Int(i128::from(id) + 1));
        let end = tuple::encode(&parts);
        let mut cursor = self.store.range(&base, &end, true);
        let Some((k, v)) = cursor.next(1).into_iter().next() else {
            return Ok(None);
        };
        let start = run_start(&k, &base)?;
        // A cached run is newer than the store's.
        let len = match self.runs.entry(start) {
            Entry::Occupied(run) => run.get().len(),
            Entry::Vacant(slot) => slot.insert(Run::decode(&v)?).len(),
        };
        Ok((id - start < len).then_some(start))
    }

    fn run(&mut self, start: u64) -> Result<&mut Run, StorageError> {
        if !self.runs.contains_key(&start) {
            let bytes = self.store.get(&key(self.path, RUN, Some(start))?);
            let run = Run::decode(&bytes.ok_or_else(missing)?)?;
            let _old = self.runs.insert(start, run);
        }
        self.runs.get_mut(&start).ok_or_else(missing)
    }

    fn leaf_of(&mut self, start: u64) -> Result<NodeRef, StorageError> {
        if let Some(leaf) = self.links.get(&start) {
            return Ok(*leaf);
        }
        let bytes = self.store.get(&key(self.path, LINK, Some(start))?);
        let leaf: NodeRef = value::from_slice(&bytes.ok_or_else(missing)?)?;
        let _old = self.links.insert(start, leaf);
        Ok(leaf)
    }

    fn node(&mut self, r: NodeRef) -> Result<&Node, StorageError> {
        self.load_node(r)?;
        self.nodes.get(&r).ok_or_else(missing)
    }

    /// A node, to change in place: it will be written.
    fn node_mut(&mut self, r: NodeRef) -> Result<&mut Node, StorageError> {
        self.load_node(r)?;
        let _new = self.dirty_nodes.insert(r);
        self.nodes.get_mut(&r).ok_or_else(missing)
    }

    fn load_node(&mut self, r: NodeRef) -> Result<(), StorageError> {
        if self.nodes.contains_key(&r) {
            return Ok(());
        }
        let bytes = self.store.get(&node_key(self.path, r)?);
        let node = match (bytes, r) {
            (Some(bytes), _) => value::from_slice(&bytes)?,
            (None, None) => Node::empty_root(),
            (None, Some(_)) => return Err(missing()),
        };
        let _old = self.nodes.insert(r, node);
        Ok(())
    }

    fn put_node(&mut self, r: NodeRef, node: Node) {
        let _new = self.dirty_nodes.insert(r);
        let _old = self.nodes.insert(r, node);
    }

    fn set_link(&mut self, start: u64, leaf: NodeRef) {
        let _new = self.dirty_links.insert(start);
        let _old = self.links.insert(start, leaf);
    }

    /// Point the children of `node` (now at `r`) at it.
    fn adopt(&mut self, node: &Node, r: NodeRef) -> Result<(), StorageError> {
        for (child, _) in &node.items {
            if node.leaf {
                self.set_link(*child, r);
            } else {
                self.node_mut(Some(*child))?.parent = r;
            }
        }
        Ok(())
    }

    /// Change the live count of the run `start` in `leaf` by `delta`, and its ancestors'.
    fn shift_item(&mut self, leaf: NodeRef, start: u64, delta: i64) -> Result<(), StorageError> {
        let node = self.node_mut(leaf)?;
        let i = node.position(start)?;
        node.items[i].1 = shift(node.items[i].1, delta)?;
        self.bump(leaf, delta)
    }

    /// Change the live counts above `r` by `delta`.
    fn bump(&mut self, mut r: NodeRef, delta: i64) -> Result<(), StorageError> {
        if delta == 0 {
            return Ok(());
        }
        for _ in 0..=MAX_DEPTH {
            let Some(id) = r else {
                return Ok(());
            };
            let parent = self.node(r)?.parent;
            let p = self.node_mut(parent)?;
            let i = p.position(id)?;
            p.items[i].1 = shift(p.items[i].1, delta)?;
            r = parent;
        }
        Err(too_deep())
    }

    /// Insert `items` into `leaf` at `at`, then split what overflows.
    fn insert_items(
        &mut self,
        leaf: NodeRef,
        at: usize,
        items: Vec<(u64, u64)>,
    ) -> Result<(), StorageError> {
        let delta: u64 = items.iter().map(|(_, live)| live).sum();
        for (start, _) in &items {
            self.set_link(*start, leaf);
        }
        let node = self.node_mut(leaf)?;
        let _old: Vec<_> = node.items.splice(at..at, items).collect();
        self.bump(leaf, i64::try_from(delta).map_err(too_long)?)?;
        self.split(leaf)
    }

    /// Split `r` into nodes of at most [`FANOUT`] items if it has more: a root moves its
    /// items into new children, any other node gives its later items to new siblings.
    fn split(&mut self, r: NodeRef) -> Result<(), StorageError> {
        let node = self.node(r)?.clone();
        if node.items.len() <= FANOUT {
            return Ok(());
        }
        let pieces = even_chunks(&node.items, FANOUT);
        let mut made = Vec::new();
        let keep = usize::from(r.is_some());
        for piece in &pieces[keep..] {
            let id = self.mint(1)?;
            let child = Node {
                leaf: node.leaf,
                parent: if r.is_some() { node.parent } else { None },
                items: piece.clone(),
            };
            self.adopt(&child, Some(id))?;
            made.push((id, child.live()));
            self.put_node(Some(id), child);
        }
        let Some(id) = r else {
            self.put_node(
                None,
                Node {
                    leaf: false,
                    parent: None,
                    items: made,
                },
            );
            return self.split(None);
        };
        let kept = Node {
            items: pieces[0].clone(),
            ..node.clone()
        };
        let kept_live = kept.live();
        self.put_node(r, kept);
        let parent = self.node_mut(node.parent)?;
        let i = parent.position(id)?;
        parent.items[i].1 = kept_live;
        let _old: Vec<_> = parent.items.splice(i + 1..i + 1, made).collect();
        self.split(node.parent)
    }

    /// The leaf, item and live offset in its run of the live character at `index`.
    fn seek(&mut self, index: u64) -> Result<Option<(NodeRef, usize, u64)>, StorageError> {
        let mut r = None;
        let mut k = index;
        for _ in 0..=MAX_DEPTH {
            let node = self.node(r)?;
            let mut found = None;
            for (i, (key, live)) in node.items.iter().enumerate() {
                if k < *live {
                    found = Some((i, *key));
                    break;
                }
                k -= live;
            }
            let Some((i, key)) = found else {
                return Ok(None);
            };
            if node.leaf {
                return Ok(Some((r, i, k)));
            }
            r = Some(key);
        }
        Err(too_deep())
    }

    fn first_leaf(&mut self) -> Result<NodeRef, StorageError> {
        self.leftmost(None)
    }

    fn leftmost(&mut self, mut r: NodeRef) -> Result<NodeRef, StorageError> {
        for _ in 0..=MAX_DEPTH {
            let node = self.node(r)?;
            if node.leaf {
                return Ok(r);
            }
            r = Some(node.items.first().ok_or_else(out_of_step)?.0);
        }
        Err(too_deep())
    }

    /// The leaf after `r` in document order.
    fn next_leaf(&mut self, mut r: NodeRef) -> Result<Option<NodeRef>, StorageError> {
        for _ in 0..=MAX_DEPTH {
            let Some(id) = r else {
                return Ok(None);
            };
            let parent = self.node(r)?.parent;
            let p = self.node(parent)?;
            let i = p.position(id)?;
            if let Some((next, _)) = p.items.get(i + 1) {
                let next = Some(*next);
                return Ok(Some(self.leftmost(next)?));
            }
            r = parent;
        }
        Err(too_deep())
    }
}

/// `items` cut into the fewest pieces of at most `max`, as even as can be.
fn even_chunks(items: &[(u64, u64)], max: usize) -> Vec<Vec<(u64, u64)>> {
    let pieces = items.len().div_ceil(max);
    let (size, extra) = (items.len() / pieces, items.len() % pieces);
    let mut out = Vec::with_capacity(pieces);
    let mut at = 0;
    for n in 0..pieces {
        let len = size + usize::from(n < extra);
        out.push(items[at..at + len].to_vec());
        at += len;
    }
    out
}

/// Every run of the document at `path`, in document order: the root, the other nodes and
/// the runs, each read a batch at a time.
pub(super) fn sequence<S: Store>(
    path: &KeyPath,
    store: &S,
) -> Result<Vec<(u64, Run)>, StorageError> {
    let root: Node = match store.get(&node_key(path, None)?) {
        Some(bytes) => value::from_slice(&bytes)?,
        None => return Ok(Vec::new()),
    };
    let mut nodes = BTreeMap::new();
    if !root.leaf {
        let base = key(path, NODE, None)?;
        scan(store, &base, |k, v| {
            let _old = nodes.insert(run_start(k, &base)?, value::from_slice::<Node>(v)?);
            Ok(())
        })?;
    }
    let mut runs = BTreeMap::new();
    let base = key(path, RUN, None)?;
    scan(store, &base, |k, v| {
        let _old = runs.insert(run_start(k, &base)?, Run::decode(v)?);
        Ok(())
    })?;
    let mut out = Vec::with_capacity(runs.len());
    // Depth-first, a stack of (node, next item).
    let mut stack: Vec<(&Node, usize)> = vec![(&root, 0)];
    while let Some((node, i)) = stack.pop() {
        let Some((key, _)) = node.items.get(i) else {
            continue;
        };
        stack.push((node, i + 1));
        if node.leaf {
            let run = runs.remove(key).ok_or_else(missing)?;
            out.push((*key, run));
        } else {
            if stack.len() > MAX_DEPTH {
                return Err(too_deep());
            }
            stack.push((nodes.get(key).ok_or_else(missing)?, 0));
        }
    }
    Ok(out)
}

fn scan<S: Store>(
    store: &S,
    base: &[u8],
    mut each: impl FnMut(&[u8], &[u8]) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    let (start, end) = bounds(base);
    let mut cursor = store.range(&start, &end, false);
    loop {
        let batch = cursor.next(MAX_RANGE_BATCH);
        if batch.is_empty() {
            return Ok(());
        }
        for (k, v) in batch {
            each(&k, &v)?;
        }
    }
}

fn node_key(path: &KeyPath, r: NodeRef) -> Result<Vec<u8>, StorageError> {
    match r {
        None => key(path, ROOT, None),
        Some(id) => key(path, NODE, Some(id)),
    }
}

/// The id in a `(name, kind, id)` key under `base`.
fn run_start(k: &[u8], base: &[u8]) -> Result<u64, StorageError> {
    match key_after(k, base)?.as_slice() {
        [KeyPart::Int(id)] => {
            u64::try_from(*id).map_err(|_| StorageError::Decode("a negative text id".into()))
        }
        _ => Err(StorageError::Decode("not a text key".into())),
    }
}

fn missing() -> StorageError {
    StorageError::Decode("a text row is missing".into())
}

fn out_of_step() -> StorageError {
    StorageError::Decode("text tree out of step".into())
}

fn too_deep() -> StorageError {
    StorageError::Decode("text tree too deep".into())
}

fn too_long(_: core::num::TryFromIntError) -> StorageError {
    StorageError::Encode("text too long".into())
}
