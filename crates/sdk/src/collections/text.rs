//! `Text`: a Fugue sequence of characters over the store (v1's `FugueText`).
//!
//! Tree-Fugue, Algorithm 1 of Weidner, Gentle and Kleppmann, *The Art of the Fugue*
//! (arXiv 2305.00583), as v1's `calimero-storage` (0.x, since removed) implemented it. One
//! leader applies every edit, so there is no merge; Fugue is here for what editors send: an
//! edit typed against a view that is no longer current. Every character has a stable id,
//! an insert names the id of the character it follows, and a delete names ids, so an edit
//! lands where its author meant even after others edited around it, and two runs typed
//! after the same character never interleave.
//!
//! Applied one at a time, a Fugue insert puts the new characters right after the one it
//! names, tombstones included (the right child of a node without one, else the newest left
//! child of its successor: both are the next place in document order). So the document is a
//! sequence with tombstones, and it is stored as one:
//!
//! - runs of characters with consecutive ids, one row each, in a counted B+ tree that finds a
//!   run by id or by live index in O(log runs) host calls (`text/runs.rs`);
//! - `(name, "n")` is the number of live characters, kept with `Store::add`;
//! - `(name, "i")` is the next id, shared with `RichText`'s marks and the tree's nodes.
//!
//! An edit that would pass a run's limits (`MAX_RUN_WRITES`, `MAX_RUN_BYTES`) is refused
//! with `EditTooLarge` before any write.

use crate::app::StateField;
use crate::commons::{clear_prefix, collection_key, read_count, KeyPath};
use crate::encoding::key::KeyPart;
use crate::error::StorageError;
use crate::store::{DefaultStore, Store, MAX_RUN_BYTES, MAX_RUN_WRITES};

mod runs;

pub(crate) use runs::Run;
use runs::{Doc, LINK, NODE, ROOT, RUN};

/// The live length.
const LEN: &str = "n";
/// The next id.
const NEXT: &str = "i";
/// What the host charges for the value `Store::add` leaves at most: a CBOR integer.
const ADD_VALUE_BYTES: usize = 9;

/// An edit, applied to a [`Doc`].
pub(crate) enum Edit<'e> {
    InsertAfter(Option<u64>, &'e str),
    Insert(u64, &'e str),
    DeleteIds(&'e [u64]),
    Delete(u64, u64),
}

/// What an edit answers.
pub(crate) enum Done {
    Ids(Vec<u64>),
    Count(u64),
}

impl Done {
    fn ids(self) -> Vec<u64> {
        match self {
            Done::Ids(ids) => ids,
            Done::Count(_) => Vec::new(),
        }
    }

    fn count(self) -> u64 {
        match self {
            Done::Count(n) => n,
            Done::Ids(ids) => ids.len() as u64,
        }
    }
}

enum Write {
    Put(Vec<u8>, Vec<u8>),
    Add(Vec<u8>, i64),
}

/// The writes an edit needs, counted as the host counts them, made only once they fit.
#[derive(Default)]
pub(crate) struct Plan {
    writes: Vec<Write>,
    bytes: usize,
}

impl Plan {
    fn put(&mut self, k: Vec<u8>, v: Vec<u8>) {
        self.bytes += k.len() + v.len();
        self.writes.push(Write::Put(k, v));
    }

    fn add(&mut self, k: Vec<u8>, n: u64) -> Result<(), StorageError> {
        let delta = i64::try_from(n).map_err(|_| StorageError::Encode("too long".into()))?;
        self.bytes += k.len() + ADD_VALUE_BYTES;
        self.writes.push(Write::Add(k, delta));
        Ok(())
    }

    fn sub(&mut self, k: Vec<u8>, n: u64) -> Result<(), StorageError> {
        let delta = i64::try_from(n).map_err(|_| StorageError::Encode("too long".into()))?;
        self.bytes += k.len() + ADD_VALUE_BYTES;
        self.writes.push(Write::Add(k, -delta));
        Ok(())
    }

    fn fits(&self) -> bool {
        self.writes.len() <= MAX_RUN_WRITES && self.bytes <= MAX_RUN_BYTES
    }

    /// `EditTooLarge` unless the writes fit one run.
    fn check(&self) -> Result<(), StorageError> {
        if self.fits() {
            return Ok(());
        }
        Err(StorageError::EditTooLarge {
            writes: self.writes.len(),
            bytes: self.bytes,
        })
    }

    fn apply<S: Store>(self, store: &mut S) -> Result<(), StorageError> {
        for write in self.writes {
            match write {
                Write::Put(k, v) => store.put(&k, &v)?,
                Write::Add(k, delta) => store.add(&k, delta)?,
            }
        }
        Ok(())
    }
}

/// The key `(path.., kind)` or `(path.., kind, id)`.
fn key(path: &KeyPath, kind: &str, id: Option<u64>) -> Result<Vec<u8>, StorageError> {
    match id {
        None => collection_key(path, kind, &[]),
        Some(id) => collection_key(path, kind, &[KeyPart::Int(id.into())]),
    }
}

/// A Fugue text; see the module docs.
pub struct Text<S: Store = DefaultStore> {
    pub(crate) name: KeyPath,
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
            name: KeyPath::root(name),
            store,
        }
    }

    /// Reserve `n` fresh ids; returns the first.
    pub(crate) fn mint(&mut self, n: u64) -> Result<u64, StorageError> {
        let key = key(&self.name, NEXT, None)?;
        let first = read_count(&self.store, &key, self.name.label())?;
        let delta = i64::try_from(n).map_err(|_| StorageError::Encode("too many ids".into()))?;
        self.store.add(&key, delta)?;
        Ok(first)
    }

    /// Apply `edit`: the rows it touches are read into a [`Doc`], changed there, and written
    /// once they are known to fit one run.
    fn edit(&mut self, edit: &Edit<'_>) -> Result<Done, StorageError> {
        let mut doc = Doc::open(&self.name, &self.store);
        let done = doc.edit(edit)?;
        let plan = doc.plan()?;
        plan.check()?;
        plan.apply(&mut self.store)?;
        Ok(done)
    }

    /// The number of live characters: one read of the stored count.
    pub fn len(&self) -> Result<u64, StorageError> {
        read_count(&self.store, &key(&self.name, LEN, None)?, self.name.label())
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
        let mut out = Vec::new();
        for (start, run) in self.sequence()? {
            for (id, cell) in (start..).zip(run.cells) {
                if let Some(ch) = cell {
                    out.push((id, ch));
                }
            }
        }
        Ok(out)
    }

    /// Every run in document order, deleted characters included.
    pub(crate) fn sequence(&self) -> Result<Vec<(u64, Run)>, StorageError> {
        runs::sequence(&self.name, &self.store)
    }

    /// `first` and `last` in document order (either may come first), and the id of the
    /// character after the later one; `UnknownId` when either never existed.
    pub(crate) fn ordered(
        &self,
        first: u64,
        last: u64,
    ) -> Result<(u64, u64, Option<u64>), StorageError> {
        Doc::open(&self.name, &self.store).ordered(first, last)
    }

    /// Insert `text` right after the character `after` (`None`: at the start), returning the
    /// new characters' ids in order. `after` may be deleted: the text still lands where it
    /// stood. `UnknownId` when there is no such character.
    pub fn insert_after(
        &mut self,
        after: Option<u64>,
        text: &str,
    ) -> Result<Vec<u64>, StorageError> {
        Ok(self.edit(&Edit::InsertAfter(after, text))?.ids())
    }

    /// Insert `text` before the live character at `index` (`index == len` appends), returning
    /// the new ids; `OutOfBounds` past the end.
    pub fn insert(&mut self, index: u64, text: &str) -> Result<Vec<u64>, StorageError> {
        Ok(self.edit(&Edit::Insert(index, text))?.ids())
    }

    /// Delete the characters `ids` names; ids already deleted are skipped. Returns how many
    /// were live; `UnknownId` (and nothing deleted) when one never existed.
    pub fn delete_ids(&mut self, ids: &[u64]) -> Result<u64, StorageError> {
        Ok(self.edit(&Edit::DeleteIds(ids))?.count())
    }

    /// Delete up to `count` live characters from `index`; returns how many it deleted.
    pub fn delete(&mut self, index: u64, count: u64) -> Result<u64, StorageError> {
        Ok(self.edit(&Edit::Delete(index, count))?.count())
    }

    /// The id of the live character at `index`.
    pub fn id_at(&self, index: u64) -> Result<Option<u64>, StorageError> {
        Doc::open(&self.name, &self.store).id_at(index)
    }

    /// How many live characters precede the character `id` (deleted or not): where it is,
    /// or was, in the text. `None` when there is no such character.
    pub fn index_of(&self, id: u64) -> Result<Option<u64>, StorageError> {
        Doc::open(&self.name, &self.store).index_of(id)
    }

    /// Remove every character, tombstones included, a batch at a time. Ids are not reused.
    pub fn clear(&mut self) -> Result<(), StorageError> {
        for kind in [RUN, LINK, NODE] {
            let base = key(&self.name, kind, None)?;
            let _removed = clear_prefix(&mut self.store, &base)?;
        }
        for kind in [ROOT, LEN] {
            self.store.delete(&key(&self.name, kind, None)?)?;
        }
        Ok(())
    }
}

impl<S: Store> crate::collections::Nestable<S> for Text<S> {
    fn at(path: KeyPath, store: S) -> Self {
        Self { name: path, store }
    }
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
    fn an_insert_inside_a_run_splits_it_and_ids_stay() {
        let (mut t, _) = text();
        let ab = t.insert(0, "abcdef").unwrap();
        let x = t.insert_after(Some(ab[2]), "X").unwrap();
        assert_eq!(t.read().unwrap(), "abcXdef");
        let expect: Vec<(u64, char)> = vec![
            (ab[0], 'a'),
            (ab[1], 'b'),
            (ab[2], 'c'),
            (x[0], 'X'),
            (ab[3], 'd'),
            (ab[4], 'e'),
            (ab[5], 'f'),
        ];
        assert_eq!(t.chars().unwrap(), expect, "the split keeps every id");
        assert_eq!(t.index_of(ab[3]).unwrap(), Some(4));
        assert_eq!(t.id_at(4).unwrap(), Some(ab[3]));
        // Both halves still take edits by id.
        let _ = t.insert_after(Some(ab[5]), "!").unwrap();
        let _ = t.delete_ids(&[ab[0], ab[4]]).unwrap();
        assert_eq!(t.read().unwrap(), "bcXdf!");
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
            store.op_counts(&key(&t.name, LEN, None).unwrap()).0,
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
        // The run, its leaf link, the root, the count and the next id.
        assert_eq!(store.len(), 5);
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

    #[test]
    fn a_paste_past_a_run_s_limits_is_refused_before_any_write() {
        let (mut t, store) = text();
        let _ = t.insert(0, "keep").unwrap();
        let writes = store.writes();
        let huge = "x".repeat(MAX_RUN_BYTES + 1);
        assert!(matches!(
            t.insert(2, &huge),
            Err(StorageError::EditTooLarge { .. })
        ));
        assert_eq!(store.writes(), writes, "nothing written");
        assert_eq!(t.read().unwrap(), "keep");
    }

    /// Random edits by id and by index against a reference model: the document as a list
    /// of `(id, char)` with tombstones, where an insert lands right after the id it names
    /// (what Fugue does with edits applied one at a time).
    #[test]
    fn random_edits_match_a_reference_model() {
        let (mut t, _) = text();
        let mut model: Vec<(u64, Option<char>)> = Vec::new();
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound.max(1)
        };
        let live = |model: &[(u64, Option<char>)]| -> Vec<(u64, char)> {
            model
                .iter()
                .filter_map(|(id, c)| Some((*id, (*c)?)))
                .collect()
        };
        for step in 0..1_500 {
            let len = t.len().unwrap();
            let roll = next(10);
            if roll < 5 || model.is_empty() {
                let after = if model.is_empty() || next(8) == 0 {
                    None
                } else {
                    Some(model[next(model.len() as u64) as usize].0)
                };
                let text: String = (0..1 + next(4))
                    .map(|_| char::from(b'a' + next(26) as u8))
                    .collect();
                let ids = t.insert_after(after, &text).unwrap();
                let at = after.map_or(0, |a| model.iter().position(|x| x.0 == a).unwrap() + 1);
                let new = ids.into_iter().zip(text.chars().map(Some));
                let _old: Vec<_> = model.splice(at..at, new).collect();
            } else if roll < 7 {
                let at = next(len + 1);
                let ids = t.insert(at, "Q").unwrap();
                let pos = match at.checked_sub(1) {
                    None => 0,
                    Some(i) => {
                        let id = live(&model)[i as usize].0;
                        model.iter().position(|x| x.0 == id).unwrap() + 1
                    }
                };
                model.insert(pos, (ids[0], Some('Q')));
            } else if roll < 9 {
                let picked: Vec<u64> = (0..1 + next(3))
                    .map(|_| model[next(model.len() as u64) as usize].0)
                    .collect();
                let mut killed = 0;
                for cell in model.iter_mut().filter(|x| picked.contains(&x.0)) {
                    killed += u64::from(cell.1.take().is_some());
                }
                assert_eq!(t.delete_ids(&picked).unwrap(), killed);
            } else {
                let (at, n) = (next(len + 1), next(6));
                let doomed: Vec<u64> = live(&model)
                    .into_iter()
                    .skip(at as usize)
                    .take(n as usize)
                    .map(|x| x.0)
                    .collect();
                for cell in model.iter_mut().filter(|x| doomed.contains(&x.0)) {
                    cell.1 = None;
                }
                assert_eq!(t.delete(at, n).unwrap(), doomed.len() as u64);
            }
            if step % 50 == 0 || step == 1_499 {
                assert_eq!(t.chars().unwrap(), live(&model), "after step {step}");
                assert_eq!(t.len().unwrap(), live(&model).len() as u64);
            }
        }
        // Index lookups agree with the model, deleted characters included.
        let chars = live(&model);
        for (i, (id, _)) in chars.iter().enumerate().step_by(7) {
            assert_eq!(t.id_at(i as u64).unwrap(), Some(*id));
            assert_eq!(t.index_of(*id).unwrap(), Some(i as u64));
        }
        assert_eq!(t.id_at(chars.len() as u64).unwrap(), None);
        for (pos, (id, c)) in model.iter().enumerate().step_by(5) {
            let before = model[..pos].iter().filter(|x| x.1.is_some()).count() as u64;
            assert_eq!(t.index_of(*id).unwrap(), Some(before), "{c:?}");
        }
    }

    #[test]
    fn a_deep_tree_answers_by_index_and_id() {
        let (mut t, _) = text();
        let mut model: Vec<(u64, char)> = Vec::new();
        // Each character typed at the start is a run of its own: 5,000 runs, a tree of
        // several levels.
        for i in 0..5_000_u32 {
            let ch = char::from(b'a' + (i % 26) as u8);
            let id = t.insert(0, &ch.to_string()).unwrap()[0];
            model.insert(0, (id, ch));
        }
        assert_eq!(t.chars().unwrap(), model);
        for i in (0..5_000).step_by(97) {
            assert_eq!(t.id_at(i as u64).unwrap(), Some(model[i].0));
            assert_eq!(t.index_of(model[i].0).unwrap(), Some(i as u64));
        }
        // A range across many leaves.
        assert_eq!(t.delete(10, 4_000).unwrap(), 4_000);
        let _ = model.drain(10..4_010);
        assert_eq!(t.chars().unwrap(), model);
        assert_eq!(t.index_of(model[20].0).unwrap(), Some(20));
    }
}
