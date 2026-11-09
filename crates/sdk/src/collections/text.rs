//! `Text`: a Fugue sequence of characters over the store (v1's `FugueText`).
//!
//! Tree-Fugue, Algorithm 1 of Weidner, Gentle and Kleppmann, *The Art of the Fugue*
//! (arXiv 2305.00583), as v1's `crates/storage/src/collections/fugue.rs` implements it. One
//! leader applies every edit, so there is no merge; Fugue is here for what editors send: an
//! edit typed against a view that is no longer current. Every character has a stable id,
//! an insert names the id of the character it follows, and a delete names ids, so an edit
//! lands where its author meant even after others edited around it, and two runs typed
//! after the same character never interleave.
//!
//! Key layout:
//!
//! - `(name, "t", id)` is a character: `{ch, parent, right}`, `ch` null once deleted (a
//!   tombstone keeps its place, so ids that name it still resolve);
//! - `(name, "n")` is the number of live characters, kept with `Store::add`;
//! - `(name, "i")` is the next id, shared with `RichText`'s marks.
//!
//! The document order is the depth-first traversal: a node's left children by ascending id,
//! the node, then its right children by ascending id. Every read and edit but `len` loads
//! the text's nodes, a batch of at most 1,000 per host read.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::app::StateField;
use crate::commons::{bounds, clear_prefix, collection_key, key_after, read_count};
use crate::encoding::key::KeyPart;
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{DefaultStore, RangeCursor, Store, MAX_RANGE_BATCH};

/// One stored character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Node {
    /// The character; `None` once deleted.
    pub(crate) ch: Option<char>,
    /// The node it hangs off; `None` is the root.
    pub(crate) parent: Option<u64>,
    /// Whether it is a right child.
    pub(crate) right: bool,
}

/// The loaded tree of a text.
#[derive(Default)]
pub(crate) struct Tree {
    pub(crate) nodes: BTreeMap<u64, Node>,
    children: BTreeMap<(Option<u64>, bool), BTreeSet<u64>>,
}

impl Tree {
    fn attach(&mut self, id: u64, node: Node) {
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

    /// Every node id in document order, tombstones included. Iterative: sequential typing
    /// builds a right spine as deep as the text is long.
    pub(crate) fn order(&self) -> Vec<u64> {
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

    pub(crate) fn is_live(&self, id: u64) -> bool {
        self.nodes.get(&id).is_some_and(|n| n.ch.is_some())
    }

    /// Where a new character following `left` goes (Fugue's local insert): the right child
    /// of `left` when it has none, else the left child of `left`'s successor in `order`.
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
}

/// A Fugue text; see the module docs.
pub struct Text<S: Store = DefaultStore> {
    pub(crate) name: String,
    pub(crate) store: S,
}

impl<S: Store> StateField for Text<S> {
    const KIND: &'static str = "text";
}

impl<S: Store + Default> Text<S> {
    /// A text on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<S: Store> Text<S> {
    /// A text on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: name.into(),
            store,
        }
    }

    fn node_key(&self, id: u64) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "t", &[KeyPart::Int(id.into())])
    }

    fn len_key(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "n", &[])
    }

    fn next_id_key(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "i", &[])
    }

    /// Reserve `n` fresh ids; returns the first.
    pub(crate) fn mint(&mut self, n: u64) -> Result<u64, StorageError> {
        let key = self.next_id_key()?;
        let first = read_count(&self.store, &key, &self.name)?;
        let delta = i64::try_from(n).map_err(|_| StorageError::Encode("too many ids".into()))?;
        self.store.add(&key, delta);
        Ok(first)
    }

    /// Every node, read a batch at a time.
    pub(crate) fn load(&self) -> Result<Tree, StorageError> {
        let base = collection_key(&self.name, "t", &[])?;
        let (start, end) = bounds(&base);
        let mut cursor = self.store.range(&start, &end, false);
        let mut tree = Tree::default();
        loop {
            let batch = cursor.next(MAX_RANGE_BATCH);
            if batch.is_empty() {
                return Ok(tree);
            }
            for (k, v) in batch {
                let id = match key_after(&k, &base)?.as_slice() {
                    [KeyPart::Int(id)] => u64::try_from(*id)
                        .map_err(|_| StorageError::Decode("a negative node id".into()))?,
                    _ => return Err(StorageError::Decode("not a node key".into())),
                };
                tree.attach(id, value::from_slice(&v)?);
            }
        }
    }

    /// The number of live characters: one read of the stored count.
    pub fn len(&self) -> Result<u64, StorageError> {
        read_count(&self.store, &self.len_key()?, &self.name)
    }

    /// Whether the text has no live characters.
    pub fn is_empty(&self) -> Result<bool, StorageError> {
        Ok(self.len()? == 0)
    }

    /// The text as a string.
    pub fn read(&self) -> Result<String, StorageError> {
        Ok(self.chars()?.into_iter().map(|(_, c)| c).collect())
    }

    /// The live characters with their ids, in order: what an editor anchors its edits to.
    pub fn chars(&self) -> Result<Vec<(u64, char)>, StorageError> {
        let tree = self.load()?;
        Ok(tree
            .order()
            .into_iter()
            .filter_map(|id| Some((id, tree.nodes.get(&id)?.ch?)))
            .collect())
    }

    /// Insert `text` right after the character `after` (`None`: at the start), returning the
    /// new characters' ids in order. `after` may be deleted: the text still lands where it
    /// stood. `UnknownId` when there is no such character.
    pub fn insert_after(
        &mut self,
        after: Option<u64>,
        text: &str,
    ) -> Result<Vec<u64>, StorageError> {
        let tree = self.load()?;
        self.insert_in(&tree, after, text)
    }

    pub(crate) fn insert_in(
        &mut self,
        tree: &Tree,
        after: Option<u64>,
        text: &str,
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
        let first = self.mint(count)?;
        let mut node = tree.place(&tree.order(), after);
        let mut ids = Vec::with_capacity(text.len());
        for (id, ch) in (first..).zip(text.chars()) {
            node.ch = Some(ch);
            self.store.put(&self.node_key(id)?, &value::to_vec(&node)?);
            ids.push(id);
            // A fresh node has no children, so the next character is its right child.
            node = Node {
                ch: None,
                parent: Some(id),
                right: true,
            };
        }
        let delta = i64::try_from(count).map_err(|_| StorageError::Encode("too long".into()))?;
        self.store.add(&self.len_key()?, delta);
        Ok(ids)
    }

    /// Insert `text` before the live character at `index` (`index == len` appends), returning
    /// the new ids; `OutOfBounds` past the end.
    pub fn insert(&mut self, index: u64, text: &str) -> Result<Vec<u64>, StorageError> {
        let tree = self.load()?;
        let after = match index.checked_sub(1) {
            None => None,
            Some(i) => {
                let Some(id) = live_at(&tree, i) else {
                    let len = self.len()?;
                    return Err(StorageError::OutOfBounds { index, len });
                };
                Some(id)
            }
        };
        self.insert_in(&tree, after, text)
    }

    /// Delete the characters `ids` names; ids already deleted are skipped. Returns how many
    /// were live; `UnknownId` (and nothing deleted) when one never existed.
    pub fn delete_ids(&mut self, ids: &[u64]) -> Result<u64, StorageError> {
        let tree = self.load()?;
        if let Some(unknown) = ids.iter().find(|id| !tree.nodes.contains_key(id)) {
            return Err(StorageError::UnknownId(*unknown));
        }
        let live: BTreeSet<u64> = ids.iter().copied().filter(|id| tree.is_live(*id)).collect();
        self.bury(&tree, &live)
    }

    /// Delete up to `count` live characters from `index`; returns how many it deleted.
    pub fn delete(&mut self, index: u64, count: u64) -> Result<u64, StorageError> {
        let tree = self.load()?;
        let live: BTreeSet<u64> = tree
            .order()
            .into_iter()
            .filter(|id| tree.is_live(*id))
            .skip(usize::try_from(index).unwrap_or(usize::MAX))
            .take(usize::try_from(count).unwrap_or(usize::MAX))
            .collect();
        self.bury(&tree, &live)
    }

    fn bury(&mut self, tree: &Tree, ids: &BTreeSet<u64>) -> Result<u64, StorageError> {
        for id in ids {
            let Some(node) = tree.nodes.get(id) else {
                continue;
            };
            let dead = Node { ch: None, ..*node };
            self.store.put(&self.node_key(*id)?, &value::to_vec(&dead)?);
        }
        let n = ids.len() as u64;
        if n > 0 {
            let delta = i64::try_from(n).map_err(|_| StorageError::Encode("too long".into()))?;
            self.store.add(&self.len_key()?, -delta);
        }
        Ok(n)
    }

    /// The id of the live character at `index`.
    pub fn id_at(&self, index: u64) -> Result<Option<u64>, StorageError> {
        Ok(live_at(&self.load()?, index))
    }

    /// How many live characters precede the character `id` (deleted or not): where it is,
    /// or was, in the text. `None` when there is no such character.
    pub fn index_of(&self, id: u64) -> Result<Option<u64>, StorageError> {
        let tree = self.load()?;
        if !tree.nodes.contains_key(&id) {
            return Ok(None);
        }
        let before = tree
            .order()
            .into_iter()
            .take_while(|n| *n != id)
            .filter(|n| tree.is_live(*n))
            .count();
        Ok(Some(before as u64))
    }

    /// Remove every character, tombstones included, a batch at a time. Ids are not reused.
    pub fn clear(&mut self) -> Result<(), StorageError> {
        let base = collection_key(&self.name, "t", &[])?;
        let len = self.len_key()?;
        let _removed = clear_prefix(&mut self.store, &base);
        self.store.delete(&len);
        Ok(())
    }
}

/// The id of the live character at `index`.
fn live_at(tree: &Tree, index: u64) -> Option<u64> {
    let index = usize::try_from(index).ok()?;
    tree.order()
        .into_iter()
        .filter(|id| tree.is_live(*id))
        .nth(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemStore;

    fn text() -> (Text<MemStore>, MemStore) {
        let store = MemStore::new();
        (Text::with_store("doc", store.clone()), store)
    }

    #[test]
    fn typing_appends_and_reads_back() {
        let (mut t, _) = text();
        let ids = t.insert(0, "hello").unwrap();
        assert_eq!(ids.len(), 5);
        let _ = t.insert(5, " world").unwrap();
        assert_eq!(t.read().unwrap(), "hello world");
        assert_eq!(t.len().unwrap(), 11);
        assert_eq!(t.id_at(0).unwrap(), Some(ids[0]));
        assert_eq!(t.index_of(ids[4]).unwrap(), Some(4));
    }

    #[test]
    fn a_mid_document_insert_hangs_off_the_right_origin() {
        let (mut t, _) = text();
        let ab = t.insert(0, "ab").unwrap();
        let c = t.insert(1, "c").unwrap();
        assert_eq!(t.read().unwrap(), "acb");
        let tree = t.load().unwrap();
        let node = tree.nodes[&c[0]];
        assert_eq!(node.parent, Some(ab[1]), "c is b's left child");
        assert!(!node.right);
    }

    #[test]
    fn inserts_at_the_start_and_out_of_bounds() {
        let (mut t, _) = text();
        let _ = t.insert(0, "world").unwrap();
        let _ = t.insert(0, "hello ").unwrap();
        assert_eq!(t.read().unwrap(), "hello world");
        assert_eq!(
            t.insert(12, "!").unwrap_err(),
            StorageError::OutOfBounds { index: 12, len: 11 }
        );
        let _ = t.insert(11, "!").unwrap();
        assert_eq!(t.read().unwrap(), "hello world!");
    }

    #[test]
    fn deletes_tombstone_and_keep_the_count() {
        let (mut t, store) = text();
        let ids = t.insert(0, "abcdef").unwrap();
        assert_eq!(t.delete(1, 2).unwrap(), 2);
        assert_eq!(t.read().unwrap(), "adef");
        assert_eq!(
            t.delete_ids(&[ids[1], ids[3]]).unwrap(),
            1,
            "b is already gone"
        );
        assert_eq!(t.read().unwrap(), "aef");
        assert_eq!(t.len().unwrap(), 3);
        assert_eq!(
            t.delete_ids(&[999]).unwrap_err(),
            StorageError::UnknownId(999)
        );
        assert_eq!(t.read().unwrap(), "aef", "an unknown id deletes nothing");
        assert_eq!(t.index_of(ids[2]).unwrap(), Some(1), "where c was");
        assert_eq!(
            store.op_counts(&t.len_key().unwrap()).0,
            0,
            "len only by add"
        );
        assert_eq!(t.delete(10, 5).unwrap(), 0);
    }

    /// The point of Fugue here: two edits typed against the same stale view.
    #[test]
    fn concurrent_runs_after_one_character_do_not_interleave() {
        let (mut t, _) = text();
        let base = t.insert(0, "ab").unwrap();
        // Alice and Bob both saw "ab" and type after 'a', one character per call.
        let mut alice = Some(base[0]);
        let mut bob = Some(base[0]);
        for (a, b) in "123".chars().zip("xyz".chars()) {
            alice = Some(t.insert_after(alice, &a.to_string()).unwrap()[0]);
            bob = Some(t.insert_after(bob, &b.to_string()).unwrap()[0]);
        }
        assert_eq!(t.read().unwrap(), "axyz123b");
    }

    #[test]
    fn an_edit_after_a_deleted_character_lands_where_it_stood() {
        let (mut t, _) = text();
        let ids = t.insert(0, "abc").unwrap();
        // Bob deletes "b" while Alice, who still sees it, types after it.
        let _ = t.delete_ids(&[ids[1]]).unwrap();
        let _ = t.insert_after(Some(ids[1]), "X").unwrap();
        assert_eq!(t.read().unwrap(), "aXc");
        assert_eq!(
            t.insert_after(Some(77), "Y").unwrap_err(),
            StorageError::UnknownId(77)
        );
    }

    #[test]
    fn an_index_edit_from_a_stale_view_would_misplace_what_ids_keep() {
        let (mut t, _) = text();
        let ids = t.insert(0, "hello").unwrap();
        // Alice, seeing "hello", means to append after 'o'.
        let o = ids[4];
        // Meanwhile Bob prepends.
        let _ = t.insert(0, ">> ").unwrap();
        let _ = t.insert_after(Some(o), "!").unwrap();
        assert_eq!(t.read().unwrap(), ">> hello!");
    }

    #[test]
    fn a_long_text_loads_in_batches_and_is_not_recursive() {
        let (mut t, _) = text();
        let mut last = None;
        let chunk = "x".repeat(500);
        for _ in 0..6 {
            last = t.insert_after(last, &chunk).unwrap().pop();
        }
        assert_eq!(t.len().unwrap(), 3_000);
        assert_eq!(t.read().unwrap().len(), 3_000);
    }

    #[test]
    fn unicode_is_per_character() {
        let (mut t, _) = text();
        let ids = t.insert(0, "héllo 🌍").unwrap();
        assert_eq!(ids.len(), 7);
        assert_eq!(t.len().unwrap(), 7);
        let _ = t.delete(1, 1).unwrap();
        assert_eq!(t.read().unwrap(), "hllo 🌍");
    }

    #[test]
    fn clear_empties_and_ids_stay_unique() {
        let (mut t, store) = text();
        let first = t.insert(0, "abc").unwrap();
        t.clear().unwrap();
        assert_eq!(t.read().unwrap(), "");
        assert_eq!(t.len().unwrap(), 0);
        let again = t.insert(0, "d").unwrap();
        assert!(again[0] > first[2]);
        assert_eq!(store.len(), 3, "one node, the count and the next id");
    }

    #[test]
    fn random_index_edits_match_a_string() {
        let (mut t, _) = text();
        let mut model: Vec<char> = Vec::new();
        // xorshift64: a fixed, dependency-free sequence.
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };
        for _ in 0..300 {
            let len = model.len() as u64;
            if len > 0 && next(10) < 3 {
                let at = next(len);
                let removed = t.delete(at, 1 + next(3)).unwrap();
                let _ = model.drain(at as usize..(at + removed) as usize);
            } else {
                let at = next(len + 1);
                let ch = char::from(b'a' + next(26) as u8);
                let _ = t.insert(at, &ch.to_string()).unwrap();
                model.insert(at as usize, ch);
            }
        }
        assert_eq!(t.read().unwrap(), model.iter().collect::<String>());
        assert_eq!(t.len().unwrap(), model.len() as u64);
    }
}
