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
//! Applied one at a time, a Fugue insert puts the new characters right after the one it
//! names, tombstones included (the right child of a node without one, else the newest left
//! child of its successor: both are the next place in document order). So the document is a
//! sequence with tombstones, and it is stored as one:
//!
//! - runs of characters with consecutive ids, one row each, in a counted B+ tree that finds a
//!   run by id or by live index in O(log runs) host calls (`text/runs.rs`);
//! - `(name, "n")` is the number of live characters, kept with `Store::add`;
//! - `(name, "i")` is the next id, shared with `RichText`'s marks and the tree's nodes;
//! - `(name, "f")` is the layout, `{version: 2, legacyRows}`.
//!
//! A text written before runs has no `(name, "f")` and one row per character ever typed,
//! `(name, "t", id)` (`text/legacy.rs`). Reads serve it as it is. Its first edit converts it: the
//! old rows are read (as every edit used to), turned into runs and written in the new
//! layout in that same run, with the same ids. The old rows are deleted in the same run when
//! that fits the run's limits; otherwise `legacyRows` stays true and every later edit
//! deletes a batch of them. A text too fragmented to convert within one run's 10,000 writes
//! and 1 MiB (about 5,000 runs of consecutive ids: a run is two rows) stays in the old
//! layout and is edited as before, each edit loading every character.
//!
//! An edit that would pass a run's limits (`MAX_RUN_WRITES`, `MAX_RUN_BYTES`) is refused
//! with `EditTooLarge` before any write.

use serde::{Deserialize, Serialize};

use crate::app::StateField;
use crate::commons::{bounds, clear_prefix, collection_key, read_count, KeyPath};
use crate::encoding::key::KeyPart;
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{DefaultStore, RangeCursor, Store, MAX_RUN_BYTES, MAX_RUN_WRITES};

mod legacy;
mod runs;

pub(crate) use runs::Run;
use runs::{Doc, LINK, MAX_RUN_CHARS, NODE, ROOT, RUN};

/// A character in the first layout.
const LEGACY: &str = "t";
/// The live length.
const LEN: &str = "n";
/// The next id.
const NEXT: &str = "i";
/// The layout row.
const FORMAT: &str = "f";
/// The run layout's version in the layout row.
const RUNS_VERSION: u32 = 2;
/// The most rows of the first layout one later edit deletes.
const LEGACY_BATCH: usize = 1_000;
/// What the host charges for the value `Store::add` leaves at most: a CBOR integer.
const ADD_VALUE_BYTES: usize = 9;

/// The layout row, `(name, "f")`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Format {
    version: u32,
    /// Rows of the first layout are still stored, to delete.
    legacy_rows: bool,
}

/// How a text is stored.
enum Layout {
    /// One row per character (no layout row, a `(name, "t", ..)` row).
    Legacy,
    /// Runs; `None` before the first edit writes the layout row.
    Runs(Option<Format>),
}

/// An edit, as either layout applies it.
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
    Delete(Vec<u8>),
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

    fn delete(&mut self, k: Vec<u8>) {
        self.bytes += k.len();
        self.writes.push(Write::Delete(k));
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

    fn apply<S: Store>(self, store: &mut S) {
        for write in self.writes {
            match write {
                Write::Put(k, v) => store.put(&k, &v),
                Write::Delete(k) => store.delete(&k),
                Write::Add(k, delta) => store.add(&k, delta),
            }
        }
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
        self.store.add(&key, delta);
        Ok(first)
    }

    /// The layout row, or the probe for a first-layout row when there is none.
    fn layout(&self) -> Result<Layout, StorageError> {
        if let Some(bytes) = self.store.get(&key(&self.name, FORMAT, None)?) {
            let format: Format = value::from_slice(&bytes)?;
            if format.version != RUNS_VERSION {
                return Err(StorageError::Decode(format!(
                    "text layout {} is newer than this SDK",
                    format.version
                )));
            }
            return Ok(Layout::Runs(Some(format)));
        }
        let (start, end) = bounds(&key(&self.name, LEGACY, None)?);
        if self.store.range(&start, &end, false).next(1).is_empty() {
            return Ok(Layout::Runs(None));
        }
        Ok(Layout::Legacy)
    }

    /// Apply `edit`, converting a first-layout text when it fits (see the module docs).
    fn edit(&mut self, edit: &Edit<'_>) -> Result<Done, StorageError> {
        match self.layout()? {
            Layout::Runs(format) => {
                let mut doc = Doc::open(&self.name, &self.store);
                let done = doc.edit(edit)?;
                let mut plan = doc.plan()?;
                if plan.writes.is_empty() {
                    return Ok(done);
                }
                match format {
                    None => self.put_format(&mut plan, false)?,
                    Some(f) if f.legacy_rows => self.collect_legacy(&mut plan)?,
                    Some(_) => {}
                }
                plan.check()?;
                plan.apply(&mut self.store);
                Ok(done)
            }
            Layout::Legacy => self.edit_legacy(edit),
        }
    }

    fn edit_legacy(&mut self, edit: &Edit<'_>) -> Result<Done, StorageError> {
        let tree = legacy::load(&self.name, &self.store)?;
        let next_id = read_count(
            &self.store,
            &key(&self.name, NEXT, None)?,
            self.name.label(),
        )?;
        let mut doc = Doc::converted(&self.name, &self.store, tree.runs(), next_id)?;
        let done = doc.edit(edit)?;
        let mut plan = doc.plan()?;
        let mut cleanup = Plan::default();
        for id in tree.nodes.keys() {
            cleanup.delete(key(&self.name, LEGACY, Some(*id))?);
        }
        let all = plan.writes.len() + cleanup.writes.len() < MAX_RUN_WRITES
            && plan.bytes + cleanup.bytes + 64 < MAX_RUN_BYTES;
        if all {
            plan.writes.extend(cleanup.writes);
            plan.bytes += cleanup.bytes;
        }
        self.put_format(&mut plan, !all)?;
        if plan.fits() {
            plan.apply(&mut self.store);
            return Ok(done);
        }
        // Too fragmented to convert in one run: edit it in place, as before.
        let len = self.len()?;
        let (done, plan) = legacy::edit(&self.name, &tree, next_id, len, edit)?;
        plan.check()?;
        plan.apply(&mut self.store);
        Ok(done)
    }

    fn put_format(&self, plan: &mut Plan, legacy_rows: bool) -> Result<(), StorageError> {
        let format = Format {
            version: RUNS_VERSION,
            legacy_rows,
        };
        plan.put(key(&self.name, FORMAT, None)?, value::to_vec(&format)?);
        Ok(())
    }

    /// Add deletes of the first layout's rows that are left, as many as fit beside `plan`
    /// up to [`LEGACY_BATCH`]; clear `legacyRows` once none is left.
    fn collect_legacy(&self, plan: &mut Plan) -> Result<(), StorageError> {
        let base = key(&self.name, LEGACY, None)?;
        // Room for the layout row and the deletes, each key the size of `base` plus an id.
        let per = base.len() + 17;
        let room = (MAX_RUN_WRITES.saturating_sub(plan.writes.len() + 1))
            .min(MAX_RUN_BYTES.saturating_sub(plan.bytes + 64) / per)
            .min(LEGACY_BATCH);
        if room == 0 {
            return Ok(());
        }
        let (start, end) = bounds(&base);
        let want = u32::try_from(room).unwrap_or(u32::MAX);
        let batch = self.store.range(&start, &end, false).next(want);
        let done = batch.len() < room;
        for (k, _) in batch {
            plan.delete(k);
        }
        if done {
            self.put_format(plan, false)?;
        }
        Ok(())
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
        match self.layout()? {
            Layout::Runs(_) => runs::sequence(&self.name, &self.store),
            Layout::Legacy => Ok(legacy::load(&self.name, &self.store)?.runs()),
        }
    }

    /// `first` and `last` in document order (either may come first), and the id of the
    /// character after the later one; `UnknownId` when either never existed.
    pub(crate) fn ordered(
        &self,
        first: u64,
        last: u64,
    ) -> Result<(u64, u64, Option<u64>), StorageError> {
        match self.layout()? {
            Layout::Runs(_) => Doc::open(&self.name, &self.store).ordered(first, last),
            Layout::Legacy => legacy::load(&self.name, &self.store)?.ordered(first, last),
        }
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
        match self.layout()? {
            Layout::Runs(_) => Doc::open(&self.name, &self.store).id_at(index),
            Layout::Legacy => Ok(legacy::load(&self.name, &self.store)?.id_at(index)),
        }
    }

    /// How many live characters precede the character `id` (deleted or not): where it is,
    /// or was, in the text. `None` when there is no such character.
    pub fn index_of(&self, id: u64) -> Result<Option<u64>, StorageError> {
        match self.layout()? {
            Layout::Runs(_) => Doc::open(&self.name, &self.store).index_of(id),
            Layout::Legacy => Ok(legacy::load(&self.name, &self.store)?.index_of(id)),
        }
    }

    /// Remove every character, tombstones included, a batch at a time. Ids are not reused.
    pub fn clear(&mut self) -> Result<(), StorageError> {
        for kind in [LEGACY, RUN, LINK, NODE] {
            let base = key(&self.name, kind, None)?;
            let _removed = clear_prefix(&mut self.store, &base);
        }
        for kind in [ROOT, LEN, FORMAT] {
            self.store.delete(&key(&self.name, kind, None)?);
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
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::store::{MemStore, MAX_RANGE_BATCH};

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
        // The run, its leaf link, the root, the layout, the count and the next id.
        assert_eq!(store.len(), 6);
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

    // --- the first layout ------------------------------------------------------------

    /// Rows the SDK wrote before runs (one per character), dumped from that code: a
    /// `RichText` "doc" holding "hello world", two people typing "AB" and "xy" after the
    /// 'o' one character per call, 'e', 'l' and 'w' deleted, ">> " typed at the start, a
    /// bold mark over "hello" (expanding) and a link over "orld", then "!" after the 'd'.
    pub(crate) const FIXTURE: &[(&str, &str)] = &[
        ("30646f63000030690000", "15"),
        ("30646f630000306b00002080000000000000000000000000000012", "a563656e640c636b657964626f6c64657374617274006576616c756564747275656c656e64496e636c7573697665f4"),
        ("30646f630000306b00002080000000000000000000000000000013", "a563656e640a636b6579646c696e6b657374617274076576616c756561786c656e64496e636c7573697665f5"),
        ("30646f630000306e0000", "10"),
        ("30646f630000307400002080000000000000000000000000000000", "a36263686168657269676874f566706172656e74f6"),
        ("30646f630000307400002080000000000000000000000000000001", "a3626368f6657269676874f566706172656e7400"),
        ("30646f630000307400002080000000000000000000000000000002", "a3626368f6657269676874f566706172656e7401"),
        ("30646f630000307400002080000000000000000000000000000003", "a3626368616c657269676874f566706172656e7402"),
        ("30646f630000307400002080000000000000000000000000000004", "a3626368616f657269676874f566706172656e7403"),
        ("30646f630000307400002080000000000000000000000000000005", "a36263686120657269676874f566706172656e7404"),
        ("30646f630000307400002080000000000000000000000000000006", "a3626368f6657269676874f566706172656e7405"),
        ("30646f630000307400002080000000000000000000000000000007", "a3626368616f657269676874f566706172656e7406"),
        ("30646f630000307400002080000000000000000000000000000008", "a36263686172657269676874f566706172656e7407"),
        ("30646f630000307400002080000000000000000000000000000009", "a3626368616c657269676874f566706172656e7408"),
        ("30646f63000030740000208000000000000000000000000000000a", "a36263686164657269676874f566706172656e7409"),
        ("30646f63000030740000208000000000000000000000000000000b", "a36263686141657269676874f466706172656e7405"),
        ("30646f63000030740000208000000000000000000000000000000c", "a36263686178657269676874f466706172656e740b"),
        ("30646f63000030740000208000000000000000000000000000000d", "a36263686142657269676874f566706172656e740b"),
        ("30646f63000030740000208000000000000000000000000000000e", "a36263686179657269676874f566706172656e740c"),
        ("30646f63000030740000208000000000000000000000000000000f", "a3626368613e657269676874f466706172656e7400"),
        ("30646f630000307400002080000000000000000000000000000010", "a3626368613e657269676874f566706172656e740f"),
        ("30646f630000307400002080000000000000000000000000000011", "a36263686120657269676874f566706172656e7410"),
        ("30646f630000307400002080000000000000000000000000000014", "a36263686121657269676874f566706172656e740a"),
    ];

    /// What the SDK answered for [`FIXTURE`] before runs.
    pub(crate) const FIXTURE_CHARS: &[(u64, char)] = &[
        (15, '>'),
        (16, '>'),
        (17, ' '),
        (0, 'h'),
        (3, 'l'),
        (4, 'o'),
        (12, 'x'),
        (14, 'y'),
        (11, 'A'),
        (13, 'B'),
        (5, ' '),
        (7, 'o'),
        (8, 'r'),
        (9, 'l'),
        (10, 'd'),
        (20, '!'),
    ];

    pub(crate) fn fixture_store() -> MemStore {
        let mut store = MemStore::new();
        for (k, v) in FIXTURE {
            store.put(&hex::decode(k).unwrap(), &hex::decode(v).unwrap());
        }
        store
    }

    fn rows(store: &MemStore, t: &Text<MemStore>, kind: &str) -> usize {
        let (start, end) = bounds(&key(&t.name, kind, None).unwrap());
        let mut cursor = store.range(&start, &end, false);
        let mut n = 0;
        loop {
            let batch = cursor.next(MAX_RANGE_BATCH);
            if batch.is_empty() {
                return n;
            }
            n += batch.len();
        }
    }

    fn format(store: &MemStore, t: &Text<MemStore>) -> Option<Format> {
        store
            .get(&key(&t.name, FORMAT, None).unwrap())
            .map(|b| value::from_slice(&b).unwrap())
    }

    /// An edit in the first layout, made by that layout's own code.
    pub(crate) fn legacy_edit(t: &mut Text<MemStore>, edit: &Edit<'_>) -> Done {
        let tree = legacy::load(&t.name, &t.store).unwrap();
        let next = read_count(&t.store, &key(&t.name, NEXT, None).unwrap(), "doc").unwrap();
        let (done, plan) = legacy::edit(&t.name, &tree, next, t.len().unwrap(), edit).unwrap();
        plan.apply(&mut t.store);
        done
    }

    #[test]
    fn an_old_document_reads_as_it_did() {
        let store = fixture_store();
        let t = Text::with_store("doc", store.clone());
        assert_eq!(t.chars().unwrap(), FIXTURE_CHARS);
        assert_eq!(t.read().unwrap(), ">> hloxyAB orld!");
        assert_eq!(t.len().unwrap(), 16);
        assert_eq!(t.id_at(6).unwrap(), Some(12));
        assert_eq!(t.index_of(12).unwrap(), Some(6));
        assert_eq!(t.index_of(6).unwrap(), Some(11), "where 'w' was");
        assert!(format(&store, &t).is_none(), "reads write nothing");
    }

    #[test]
    fn an_old_document_converts_on_its_first_edit_with_the_same_ids() {
        let store = fixture_store();
        let mut t = Text::with_store("doc", store.clone());
        // The same edit, made by the old code on a copy.
        let mut old = Text::with_store("doc", fixture_store());
        let edit = Edit::InsertAfter(Some(4), "Z");
        let _ = legacy_edit(&mut old, &edit);

        let z = t.insert_after(Some(4), "Z").unwrap();
        assert_eq!(z, vec![21], "ids continue from the stored next id");
        assert_eq!(t.read().unwrap(), ">> hloZxyAB orld!");
        assert_eq!(t.chars().unwrap(), old.chars().unwrap());
        assert_eq!(
            format(&store, &t),
            Some(Format {
                version: RUNS_VERSION,
                legacy_rows: false
            })
        );
        assert_eq!(rows(&store, &t, LEGACY), 0, "the old rows are gone");
        assert_eq!(t.len().unwrap(), 17);
        // Every old id still names its character, deleted ones included.
        let _ = t.insert_after(Some(6), "W").unwrap();
        assert_eq!(t.read().unwrap(), ">> hloZxyAB World!");
        assert_eq!(
            t.delete_ids(&[1, 2, 15]).unwrap(),
            1,
            "e and l were deleted"
        );
        assert_eq!(t.read().unwrap(), "> hloZxyAB World!");
        assert_eq!(t.index_of(20).unwrap(), Some(16));
    }

    /// The run layout makes the same edits as the old code, step by step: every id an edit
    /// names is mapped between the two documents, whose own ids differ once the tree mints
    /// node ids.
    #[test]
    fn random_edits_match_the_old_code() {
        let mut new = Text::with_store("doc", MemStore::new());
        let mut old = Text::with_store("doc", MemStore::new());
        // Every id ever minted, as (new, old), in order.
        let mut ids: Vec<(u64, u64)> = Vec::new();
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound.max(1)
        };
        for step in 0..1_500 {
            let len = new.len().unwrap();
            let roll = next(10);
            if roll < 5 || ids.is_empty() {
                let after = if ids.is_empty() || next(8) == 0 {
                    None
                } else {
                    Some(ids[next(ids.len() as u64) as usize])
                };
                let text: String = (0..1 + next(4))
                    .map(|_| char::from(b'a' + next(26) as u8))
                    .collect();
                let a = new.insert_after(after.map(|x| x.0), &text).unwrap();
                let b = legacy_edit(&mut old, &Edit::InsertAfter(after.map(|x| x.1), &text)).ids();
                ids.extend(a.into_iter().zip(b));
            } else if roll < 7 {
                let at = next(len + 1);
                let a = new.insert(at, "Q").unwrap();
                let b = legacy_edit(&mut old, &Edit::Insert(at, "Q")).ids();
                ids.extend(a.into_iter().zip(b));
            } else if roll < 9 {
                let picked: Vec<(u64, u64)> = (0..1 + next(3))
                    .map(|_| ids[next(ids.len() as u64) as usize])
                    .collect();
                let a: Vec<u64> = picked.iter().map(|x| x.0).collect();
                let b: Vec<u64> = picked.iter().map(|x| x.1).collect();
                let n = new.delete_ids(&a).unwrap();
                assert_eq!(n, legacy_edit(&mut old, &Edit::DeleteIds(&b)).count());
            } else {
                let (at, n) = (next(len + 1), next(6));
                let done = new.delete(at, n).unwrap();
                assert_eq!(done, legacy_edit(&mut old, &Edit::Delete(at, n)).count());
            }
            if step % 50 == 0 || step == 1_499 {
                let map: BTreeMap<u64, u64> = ids.iter().copied().collect();
                let a: Vec<(u64, char)> = new
                    .chars()
                    .unwrap()
                    .into_iter()
                    .map(|(id, c)| (map[&id], c))
                    .collect();
                assert_eq!(a, old.chars().unwrap(), "after step {step}");
                assert_eq!(new.len().unwrap(), old.len().unwrap());
            }
        }
        // Index lookups agree with a scan of the characters.
        let chars = new.chars().unwrap();
        for (i, (id, _)) in chars.iter().enumerate().step_by(7) {
            assert_eq!(new.id_at(i as u64).unwrap(), Some(*id));
            assert_eq!(new.index_of(*id).unwrap(), Some(i as u64));
        }
        assert_eq!(new.id_at(chars.len() as u64).unwrap(), None);
    }

    /// An old document of `n` characters each typed at the start: `n` runs of one.
    fn fragmented_old_document(n: u64) -> (Text<MemStore>, MemStore) {
        let mut store = MemStore::new();
        let t = Text::with_store("doc", store.clone());
        for id in 0..n {
            let node = legacy::Node {
                ch: Some('a'),
                parent: id.checked_sub(1),
                right: id == 0,
            };
            store.put(
                &key(&t.name, LEGACY, Some(id)).unwrap(),
                &value::to_vec(&node).unwrap(),
            );
        }
        store.add(&key(&t.name, LEN, None).unwrap(), n as i64);
        store.add(&key(&t.name, NEXT, None).unwrap(), n as i64);
        (t, store)
    }

    #[test]
    fn an_old_document_converts_first_and_deletes_its_rows_over_later_edits() {
        let (mut t, store) = fragmented_old_document(4_000);
        let _ = t.insert(0, "x").unwrap();
        assert_eq!(
            format(&store, &t),
            Some(Format {
                version: RUNS_VERSION,
                legacy_rows: true
            }),
            "converting took most of the run"
        );
        assert_eq!(rows(&store, &t, LEGACY), 4_000);
        assert_eq!(t.read().unwrap().len(), 4_001);
        let mut edits = 0;
        while format(&store, &t).is_some_and(|f| f.legacy_rows) {
            let _ = t.insert(1, "y").unwrap();
            edits += 1;
        }
        assert_eq!(
            edits, 5,
            "a batch of 1,000 per edit, then one that finds none"
        );
        assert_eq!(rows(&store, &t, LEGACY), 0);
        assert_eq!(
            t.read().unwrap(),
            format!("x{}{}", "y".repeat(5), "a".repeat(4_000))
        );
        assert_eq!(t.id_at(6).unwrap(), Some(3_999), "the first old character");
    }

    #[test]
    fn an_old_document_too_fragmented_to_convert_is_edited_in_place() {
        let (mut t, store) = fragmented_old_document(12_000);
        let ids = t.insert(0, "x").unwrap();
        assert_eq!(ids, vec![12_000]);
        assert!(format(&store, &t).is_none(), "still the first layout");
        assert_eq!(rows(&store, &t, LEGACY), 12_001);
        assert_eq!(t.id_at(0).unwrap(), Some(12_000));
        // Deleting everything would be a write per character: refused before any write.
        let writes = store.writes();
        assert!(matches!(
            t.delete(0, 12_001),
            Err(StorageError::EditTooLarge { writes: 12_002, .. })
        ));
        assert_eq!(store.writes(), writes);
        assert_eq!(t.delete(0, 5_000).unwrap(), 5_000);
        assert_eq!(t.len().unwrap(), 7_001);
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
