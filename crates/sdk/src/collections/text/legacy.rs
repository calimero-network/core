//! The first layout: one row per character ever typed, `(name, "t", id)` holding
//! `{ch, parent, right}` (`ch` null once deleted), as Tree-Fugue's nodes.
//!
//! Documents written before the run layout keep these rows until their first edit converts
//! them (see the parent module). This module reads them, and edits a document that is too
//! fragmented to convert within one run, the way the SDK always did: by loading every node.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{key, Done, Edit, Plan, Run, LEGACY, LEN, MAX_RUN_CHARS, NEXT};
use crate::commons::{bounds, key_after, KeyPath};
use crate::encoding::key::KeyPart;
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{RangeCursor, Store, MAX_RANGE_BATCH};

/// One stored character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Node {
    /// The character; `None` once deleted.
    pub(super) ch: Option<char>,
    /// The node it hangs off; `None` is the root.
    pub(super) parent: Option<u64>,
    /// Whether it is a right child.
    pub(super) right: bool,
}

/// The loaded tree of a text in the first layout.
#[derive(Default)]
pub(super) struct Tree {
    pub(super) nodes: BTreeMap<u64, Node>,
    children: BTreeMap<(Option<u64>, bool), BTreeSet<u64>>,
}

impl Tree {
    pub(super) fn attach(&mut self, id: u64, node: Node) {
        let _new = self
            .children
            .entry((node.parent, node.right))
            .or_default()
            .insert(id);
        let _old = self.nodes.insert(id, node);
    }

    fn children(&self, parent: Option<u64>, right: bool) -> impl Iterator<Item = u64> + '_ {
        self.children
            .get(&(parent, right))
            .into_iter()
            .flat_map(|s| s.iter().copied())
    }

    /// Every node id in document order, tombstones included: a node's left children by
    /// ascending id, the node, then its right children. Iterative: sequential typing builds
    /// a right spine as deep as the text is long.
    pub(super) fn order(&self) -> Vec<u64> {
        enum Frame {
            Visit(Option<u64>),
            Emit(u64),
        }
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack = vec![Frame::Visit(None)];
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Emit(id) => out.push(id),
                Frame::Visit(id) => {
                    let right: Vec<u64> = self.children(id, true).collect();
                    stack.extend(right.into_iter().rev().map(|c| Frame::Visit(Some(c))));
                    if let Some(id) = id {
                        stack.push(Frame::Emit(id));
                    }
                    let left: Vec<u64> = self.children(id, false).collect();
                    stack.extend(left.into_iter().rev().map(|c| Frame::Visit(Some(c))));
                }
            }
        }
        out
    }

    fn is_live(&self, id: u64) -> bool {
        self.nodes.get(&id).is_some_and(|n| n.ch.is_some())
    }

    /// The live ids in document order.
    fn live(&self) -> Vec<u64> {
        self.order()
            .into_iter()
            .filter(|id| self.is_live(*id))
            .collect()
    }

    /// Where a new character following `left` goes (Fugue's local insert): the right child
    /// of `left` when it has none, else the left child of `left`'s successor in `order`.
    /// Either way it lands right after `left` in document order.
    fn place(&self, order: &[u64], left: Option<u64>) -> Node {
        if self.children(left, true).next().is_none() {
            return Node {
                ch: None,
                parent: left,
                right: true,
            };
        }
        let successor = match left {
            None => order.first().copied(),
            Some(left) => order
                .iter()
                .position(|n| *n == left)
                .and_then(|at| order.get(at + 1).copied()),
        };
        Node {
            ch: None,
            parent: successor,
            right: false,
        }
    }

    /// The document as runs, in order: consecutive characters with consecutive ids share a
    /// run of at most [`MAX_RUN_CHARS`], as the run layout stores them.
    pub(super) fn runs(&self) -> Vec<(u64, Run)> {
        let mut out: Vec<(u64, Run)> = Vec::new();
        for id in self.order() {
            let ch = self.nodes.get(&id).and_then(|n| n.ch);
            match out.last_mut() {
                Some((start, run))
                    if *start + run.len() == id && run.cells.len() < MAX_RUN_CHARS =>
                {
                    run.cells.push(ch);
                }
                _ => out.push((id, Run { cells: vec![ch] })),
            }
        }
        out
    }

    /// The live character ids from `first` to `last` (either order) as `(first, last)` in
    /// document order, and the id after the later one; `UnknownId` when either is missing.
    pub(super) fn ordered(
        &self,
        first: u64,
        last: u64,
    ) -> Result<(u64, u64, Option<u64>), StorageError> {
        let order = self.order();
        let at = |id: u64| {
            order
                .iter()
                .position(|n| *n == id)
                .ok_or(StorageError::UnknownId(id))
        };
        let (mut a, mut b) = (at(first)?, at(last)?);
        if a > b {
            core::mem::swap(&mut a, &mut b);
        }
        Ok((order[a], order[b], order.get(b + 1).copied()))
    }

    /// The id of the live character at `index`.
    pub(super) fn id_at(&self, index: u64) -> Option<u64> {
        let index = usize::try_from(index).ok()?;
        self.live().get(index).copied()
    }

    /// How many live characters precede `id`; `None` when there is no such character.
    pub(super) fn index_of(&self, id: u64) -> Option<u64> {
        if !self.nodes.contains_key(&id) {
            return None;
        }
        let before = self
            .order()
            .into_iter()
            .take_while(|n| *n != id)
            .filter(|n| self.is_live(*n))
            .count();
        Some(before as u64)
    }
}

/// Every node of the text at `path`, read a batch at a time.
pub(super) fn load<S: Store>(path: &KeyPath, store: &S) -> Result<Tree, StorageError> {
    let base = key(path, LEGACY, None)?;
    let (start, end) = bounds(&base);
    let mut cursor = store.range(&start, &end, false);
    let mut tree = Tree::default();
    loop {
        let batch = cursor.next(MAX_RANGE_BATCH);
        if batch.is_empty() {
            return Ok(tree);
        }
        for (k, v) in batch {
            tree.attach(node_id(&k, &base)?, value::from_slice(&v)?);
        }
    }
}

/// The id in a `(name, "t", id)` key under `base`.
pub(super) fn node_id(k: &[u8], base: &[u8]) -> Result<u64, StorageError> {
    match key_after(k, base)?.as_slice() {
        [KeyPart::Int(id)] => {
            u64::try_from(*id).map_err(|_| StorageError::Decode("a negative node id".into()))
        }
        _ => Err(StorageError::Decode("not a node key".into())),
    }
}

/// An edit made in the first layout, as the SDK always made it: the writes it needs, not
/// yet made. `next_id` is the stored next id and `len` the live length.
pub(super) fn edit(
    path: &KeyPath,
    tree: &Tree,
    next_id: u64,
    len: u64,
    edit: &Edit<'_>,
) -> Result<(Done, Plan), StorageError> {
    let mut plan = Plan::default();
    let done = match *edit {
        Edit::InsertAfter(after, text) => {
            Done::Ids(insert(path, tree, next_id, after, text, &mut plan)?)
        }
        Edit::Insert(index, text) => {
            let after = match index.checked_sub(1) {
                None => None,
                Some(i) => Some(
                    tree.id_at(i)
                        .ok_or(StorageError::OutOfBounds { index, len })?,
                ),
            };
            Done::Ids(insert(path, tree, next_id, after, text, &mut plan)?)
        }
        Edit::DeleteIds(ids) => {
            if let Some(unknown) = ids.iter().find(|id| !tree.nodes.contains_key(id)) {
                return Err(StorageError::UnknownId(*unknown));
            }
            let live: BTreeSet<u64> = ids.iter().copied().filter(|id| tree.is_live(*id)).collect();
            Done::Count(bury(path, tree, &live, &mut plan)?)
        }
        Edit::Delete(index, count) => {
            let live: BTreeSet<u64> = tree
                .live()
                .into_iter()
                .skip(usize::try_from(index).unwrap_or(usize::MAX))
                .take(usize::try_from(count).unwrap_or(usize::MAX))
                .collect();
            Done::Count(bury(path, tree, &live, &mut plan)?)
        }
    };
    Ok((done, plan))
}

fn insert(
    path: &KeyPath,
    tree: &Tree,
    next_id: u64,
    after: Option<u64>,
    text: &str,
    plan: &mut Plan,
) -> Result<Vec<u64>, StorageError> {
    if let Some(after) = after {
        if !tree.nodes.contains_key(&after) {
            return Err(StorageError::UnknownId(after));
        }
    }
    let count = text.chars().count() as u64;
    if count == 0 {
        return Ok(Vec::new());
    }
    plan.add(key(path, NEXT, None)?, count)?;
    let mut node = tree.place(&tree.order(), after);
    let mut ids = Vec::with_capacity(text.len());
    for (id, ch) in (next_id..).zip(text.chars()) {
        node.ch = Some(ch);
        plan.put(key(path, LEGACY, Some(id))?, value::to_vec(&node)?);
        ids.push(id);
        // A fresh node has no children, so the next character is its right child.
        node = Node {
            ch: None,
            parent: Some(id),
            right: true,
        };
    }
    plan.add(key(path, LEN, None)?, count)?;
    Ok(ids)
}

fn bury(
    path: &KeyPath,
    tree: &Tree,
    ids: &BTreeSet<u64>,
    plan: &mut Plan,
) -> Result<u64, StorageError> {
    for id in ids {
        let Some(node) = tree.nodes.get(id) else {
            continue;
        };
        let dead = Node { ch: None, ..*node };
        plan.put(key(path, LEGACY, Some(*id))?, value::to_vec(&dead)?);
    }
    let n = ids.len() as u64;
    if n > 0 {
        plan.sub(key(path, LEN, None)?, n)?;
    }
    Ok(n)
}
