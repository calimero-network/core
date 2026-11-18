//! Iterating collections and tables: every item across pages, read a page at a time.

use calimero_sdk::collections::{Cell, Counter, Frozen, PerUser, Set, UnorderedMap, Vector};
use calimero_sdk::env::{mock, Caller};
use calimero_sdk::table::FieldSet;
use calimero_sdk::{
    CborSha256, FindOpts, Iter, KeyPart, MemStore, Page, Record, StorageError, Table, ToKeyPart,
};
use serde::{Deserialize, Serialize};

fn all<T>(it: impl Iterator<Item = Result<T, StorageError>>) -> Vec<T> {
    it.collect::<Result<_, _>>().unwrap()
}

#[test]
fn an_iterator_reads_a_page_only_when_the_loop_reaches_it() {
    let reads = std::cell::Cell::new(0);
    let fetch = |o: &FindOpts| {
        reads.set(reads.get() + 1);
        let start: u32 = o.cursor.as_deref().map_or(0, |c| c.parse().unwrap());
        let end = (start + o.limit.unwrap()).min(25);
        Ok(Page {
            items: (start..end).collect(),
            next: (end < 25).then(|| end.to_string()),
        })
    };
    let mut it = Iter::new(FindOpts::default().limit(10), fetch);
    assert_eq!(reads.get(), 0, "nothing is read before the first next");
    assert_eq!(it.by_ref().take(10).count(), 10);
    assert_eq!(reads.get(), 1);
    assert_eq!(it.next().unwrap().unwrap(), 10);
    assert_eq!(reads.get(), 2);
    assert_eq!(all(it), (11..25).collect::<Vec<_>>());
    assert_eq!(reads.get(), 3);
}

#[test]
fn a_failed_read_is_yielded_once_and_ends_the_iterator() {
    let mut it: Iter<'_, u32> = Iter::new(FindOpts::default(), |_| Err(StorageError::BadCursor));
    assert_eq!(it.next(), Some(Err(StorageError::BadCursor)));
    assert_eq!(it.next(), None);
    assert_eq!(it.next(), None);
}

#[test]
fn a_vector_iterates_past_one_page_in_both_directions() {
    let mut v: Vector<u32, MemStore> = Vector::with_store("v", MemStore::new());
    let values: Vec<u32> = (0..2_500).collect();
    assert_eq!(v.extend(&values).unwrap(), 2_500);
    assert_eq!(all(v.iter()), values);
    let back: Vec<u32> = values.iter().rev().copied().collect();
    assert_eq!(all(v.iter_with(FindOpts::default().reverse())), back);
    assert_eq!(
        all(v.iter_with(FindOpts::default().from(vec![KeyPart::Int(2_498)]))),
        [2_498, 2_499]
    );
    assert_eq!(v.first().unwrap(), Some(0));
    assert_eq!(v.last().unwrap(), Some(2_499));
}

#[test]
fn swap_remove_and_truncate_keep_the_length() {
    let mut v: Vector<String, MemStore> = Vector::with_store("v", MemStore::new());
    let empty: Vector<String, MemStore> = Vector::with_store("e", MemStore::new());
    assert_eq!(empty.last().unwrap(), None);
    for s in ["a", "b", "c", "d"] {
        let _ = v.push(&s.into()).unwrap();
    }
    assert_eq!(v.swap_remove(1).unwrap(), "b");
    assert_eq!(all(v.iter()), ["a", "d", "c"]);
    assert_eq!(v.swap_remove(2).unwrap(), "c");
    assert_eq!(
        v.swap_remove(2).unwrap_err(),
        StorageError::OutOfBounds { index: 2, len: 2 }
    );
    for s in ["e", "f", "g"] {
        let _ = v.push(&s.into()).unwrap();
    }
    assert_eq!(v.truncate(9).unwrap(), 0);
    assert_eq!(v.truncate(2).unwrap(), 3);
    assert_eq!(v.len().unwrap(), 2);
    assert_eq!(all(v.iter()), ["a", "d"]);
    assert_eq!(v.push(&"h".into()).unwrap(), 2);
    assert_eq!(v.truncate(0).unwrap(), 3);
    assert!(v.is_empty().unwrap());
}

#[test]
fn a_map_iterates_entries_keys_values_and_a_prefix() {
    let mut m: UnorderedMap<(String, u32), String, MemStore> =
        UnorderedMap::with_store("m", MemStore::new());
    let entries: Vec<((String, u32), String)> = (0..1_200)
        .map(|i| {
            (
                (if i % 2 == 0 { "ada" } else { "bob" }.to_string(), i),
                format!("v{i}"),
            )
        })
        .collect();
    assert_eq!(
        m.extend(entries.iter().map(|(k, v)| (k, v))).unwrap(),
        1_200
    );
    assert_eq!(
        m.extend([(&entries[0].0, &"again".to_string())]).unwrap(),
        0
    );
    assert_eq!(m.iter().count(), 1_200);
    let ada = [("ada".to_key_part())];
    let ada_keys: Vec<u32> = all(m.prefix_iter(&ada, FindOpts::default()))
        .into_iter()
        .map(|((_, i), _)| i)
        .collect();
    assert_eq!(ada_keys, (0..1_200).step_by(2).collect::<Vec<_>>());
    assert_eq!(all(m.keys()).first(), Some(&("ada".to_string(), 0)));
    assert_eq!(all(m.values()).last().map(String::as_str), Some("v1199"));

    let mut counts: UnorderedMap<String, u32, MemStore> =
        UnorderedMap::with_store("c", MemStore::new());
    assert_eq!(counts.get_or_insert_with(&"x".into(), || 7).unwrap(), 7);
    assert_eq!(counts.get_or_insert_with(&"x".into(), || 9).unwrap(), 7);
    assert_eq!(counts.len().unwrap(), 1);
}

#[test]
fn a_set_iterates_in_order_and_extend_counts_the_new() {
    let mut s: Set<u32, MemStore> = Set::with_store("s", MemStore::new());
    let values: Vec<u32> = (0..1_500).rev().collect();
    assert_eq!(s.extend(&values).unwrap(), 1_500);
    assert_eq!(s.extend(&[3, 5_000]).unwrap(), 1);
    let mut want: Vec<u32> = (0..1_500).collect();
    want.push(5_000);
    assert_eq!(all(s.iter()), want);
}

#[test]
fn a_cell_replaces_takes_and_updates() {
    let mut c: Cell<u32, MemStore> = Cell::with_store("c", MemStore::new());
    assert!(!c.is_set().unwrap());
    assert_eq!(c.get_or_default().unwrap(), 0);
    assert_eq!(c.update(|v| v.unwrap_or(0) + 1).unwrap(), 1);
    assert_eq!(c.update(|v| v.unwrap_or(0) + 1).unwrap(), 2);
    assert_eq!(c.replace(&10).unwrap(), Some(2));
    assert!(c.is_set().unwrap());
    assert_eq!(c.take().unwrap(), Some(10));
    assert_eq!(c.take().unwrap(), None);
    assert!(!c.is_set().unwrap());
}

#[test]
fn per_user_values_are_listed_by_account() {
    mock::reset();
    let mut p: PerUser<String, MemStore> = PerUser::with_store("p", MemStore::new());
    for account in ["carol", "alice", "bob"] {
        mock::with_mut(|s| s.caller = Caller::Account(account.into()));
        let _ = p.set(&format!("{account}'s")).unwrap();
    }
    mock::reset();
    assert!(p.contains_for("bob").unwrap());
    assert!(!p.contains_for("dave").unwrap());
    let accounts: Vec<String> = all(p.iter()).into_iter().map(|(a, _)| a).collect();
    assert_eq!(accounts, ["alice", "bob", "carol"]);
    let page = p.page(FindOpts::default().limit(2).reverse()).unwrap();
    assert_eq!(page.items[0], ("carol".to_string(), "carol's".to_string()));
    assert!(page.next.is_some());
}

#[test]
fn frozen_values_are_listed_under_their_hash() {
    let mut f: Frozen<String, CborSha256, MemStore> = Frozen::with_store("f", MemStore::new());
    let (a, _) = f.insert(&"a".into()).unwrap();
    let (b, _) = f.insert(&"b".into()).unwrap();
    let mut want = vec![(a, "a".to_string()), (b, "b".to_string())];
    want.sort();
    assert_eq!(all(f.iter()), want);
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Note {
    id: u32,
    owner: String,
}

impl Record for Note {
    const KEY: &'static [&'static str] = &["id"];
    const UNIQUE: &'static [FieldSet] = &[];
    const INDEXES: &'static [FieldSet] = &[("byOwner", &["owner"])];
    const COUNTS: &'static [FieldSet] = &[];

    fn key_parts(&self, fields: &[&str]) -> Vec<KeyPart> {
        fields
            .iter()
            .map(|f| match *f {
                "id" => KeyPart::Int(self.id.into()),
                "owner" => KeyPart::Str(self.owner.clone()),
                other => panic!("no field {other}"),
            })
            .collect()
    }
}

#[test]
fn a_table_iterates_every_record_and_an_index() {
    let mut t: Table<Note, MemStore> = Table::with_store("notes", MemStore::new());
    for id in 0..1_100 {
        let owner = if id % 3 == 0 { "ada" } else { "bob" };
        t.insert(Note {
            id,
            owner: owner.into(),
        })
        .unwrap();
    }
    let ids: Vec<u32> = all(t.iter()).into_iter().map(|n| n.id).collect();
    assert_eq!(ids, (0..1_100).collect::<Vec<_>>());
    let ada = [KeyPart::Str("ada".into())];
    let ada_ids: Vec<u32> = all(t.find_iter("byOwner", &ada, FindOpts::default().limit(50)))
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(ada_ids, (0..1_100).step_by(3).collect::<Vec<_>>());
    let unknown = t.find_iter("nope", &[], FindOpts::default()).next();
    assert_eq!(unknown, Some(Err(StorageError::UnknownName("nope".into()))));
}

#[test]
fn a_vector_inserts_removes_swaps_and_retains_by_position() {
    let mut v: Vector<u32, MemStore> = Vector::with_store("v", MemStore::new());
    let _ = v.extend(&[1, 2, 3, 4]).unwrap();
    v.insert(0, &0).unwrap();
    v.insert(5, &5).unwrap();
    v.insert(3, &9).unwrap();
    assert_eq!(all(v.iter()), [0, 1, 2, 9, 3, 4, 5]);
    assert_eq!(
        v.insert(8, &7).unwrap_err(),
        StorageError::OutOfBounds { index: 8, len: 7 }
    );
    assert_eq!(v.remove(3).unwrap(), 9);
    assert_eq!(v.remove(0).unwrap(), 0);
    assert_eq!(all(v.iter()), [1, 2, 3, 4, 5]);
    assert_eq!(v.len().unwrap(), 5);
    v.swap(0, 4).unwrap();
    v.swap(2, 2).unwrap();
    assert_eq!(all(v.iter()), [5, 2, 3, 4, 1]);
    assert_eq!(v.update(1, |x| x * 10).unwrap(), 20);
    assert!(v.contains(&20).unwrap());
    assert!(!v.contains(&2).unwrap());
    assert_eq!(v.retain(|x| x % 2 == 1).unwrap(), 2);
    assert_eq!(all(v.iter()), [5, 3, 1]);
    assert_eq!(v.len().unwrap(), 3);
    assert_eq!(
        v.update(3, |x| x).unwrap_err(),
        StorageError::OutOfBounds { index: 3, len: 3 }
    );
}

#[test]
fn a_map_updates_with_a_closure_and_retains_across_pages() {
    let mut m: UnorderedMap<u32, u32, MemStore> = UnorderedMap::with_store("m", MemStore::new());
    assert_eq!(m.update(&1, |v| Some(v.unwrap_or(0) + 1)).unwrap(), Some(1));
    assert_eq!(m.update(&1, |v| Some(v.unwrap_or(0) + 1)).unwrap(), Some(2));
    assert_eq!(m.update(&1, |_| None).unwrap(), None);
    assert!(m.is_empty().unwrap());
    let keys: Vec<u32> = (0..2_300).collect();
    let _ = m.extend(keys.iter().map(|k| (k, k))).unwrap();
    assert_eq!(m.retain(|k, _| k % 10 == 0).unwrap(), 2_070);
    assert_eq!(m.len().unwrap(), 230);
    assert_eq!(all(m.keys()), (0..2_300).step_by(10).collect::<Vec<_>>());
}

#[test]
fn a_set_retains_across_pages() {
    let mut s: Set<u32, MemStore> = Set::with_store("s", MemStore::new());
    let values: Vec<u32> = (0..2_100).collect();
    let _ = s.extend(&values).unwrap();
    assert_eq!(s.retain(|v| *v >= 2_000).unwrap(), 2_000);
    assert_eq!(s.len().unwrap(), 100);
    assert_eq!(s.first().unwrap(), Some(2_000));
}

#[test]
fn a_counter_decrements_to_zero_and_resets() {
    let mut c: Counter<MemStore> = Counter::with_store("c", MemStore::new());
    assert_eq!(c.decrement().unwrap(), 0);
    c.add(3).unwrap();
    assert_eq!(c.decrement().unwrap(), 2);
    c.reset().unwrap();
    assert_eq!(c.get().unwrap(), 0);
    assert_eq!(c.increment().unwrap(), 1);
}

#[test]
fn clearing_a_table_removes_its_indexes_and_leaves_its_neighbours() {
    let store = MemStore::new();
    let mut t: Table<Note, MemStore> = Table::with_store("notes", store.clone());
    let mut other: Table<Note, MemStore> = Table::with_store("notes2", store.clone());
    for id in 0..1_200 {
        t.insert(Note {
            id,
            owner: "ada".into(),
        })
        .unwrap();
    }
    other
        .insert(Note {
            id: 1,
            owner: "ada".into(),
        })
        .unwrap();
    assert_eq!(t.clear().unwrap(), 1_200);
    assert_eq!(t.iter().count(), 0);
    let ada = [KeyPart::Str("ada".into())];
    assert_eq!(t.find_iter("byOwner", &ada, FindOpts::default()).count(), 0);
    assert_eq!(other.iter().count(), 1);
    assert_eq!(
        store.len(),
        4,
        "the other table's record, index entry, length and its mark remain"
    );
    assert_eq!(other.len().unwrap(), 1);
}

/// What a node charges a guest is host calls and writes (at most 10,000 writes a run).
/// These pin both for the batch operations, so a change that adds a host call per
/// element shows here.
mod costs {
    use super::*;

    fn filled(n: u32) -> (Vector<u32, MemStore>, MemStore) {
        let store = MemStore::new();
        let mut v = Vector::with_store("v", store.clone());
        let values: Vec<u32> = (0..n).collect();
        let _ = v.extend(&values).unwrap();
        store.reset_calls();
        (v, store)
    }

    #[test]
    fn iterating_opens_one_cursor_and_reads_a_batch_per_call() {
        let (v, store) = filled(2_500);
        assert_eq!(v.iter().count(), 2_500);
        // Open, then batches of 1,000, 1,000 and 500, then the empty batch that ends it.
        assert_eq!(store.calls(), 5);
        store.reset_calls();
        let first_ten: Vec<u32> = all(v.iter_with(FindOpts::default().limit(10)).take(10));
        assert_eq!(first_ten, (0..10).collect::<Vec<_>>());
        assert_eq!(store.calls(), 2, "open and one batch of 10");
        store.reset_pulled();
        assert_eq!(v.iter().next().unwrap().unwrap(), 0);
        assert_eq!(store.pulled(), 1_000, "one batch, nothing read ahead of it");
    }

    #[test]
    fn extend_writes_each_value_once_and_the_length_once() {
        let store = MemStore::new();
        let mut v: Vector<u32, MemStore> = Vector::with_store("v", store.clone());
        let values: Vec<u32> = (0..6_000).collect();
        assert_eq!(v.extend(&values).unwrap(), 6_000);
        // A push per value would be 12,000 writes, past the run's 10,000.
        assert_eq!(store.writes(), 6_001);
        assert_eq!(
            store.calls(),
            6_002,
            "one length read, the puts and one add"
        );
        assert_eq!(v.len().unwrap(), 6_000);
    }

    #[test]
    fn single_element_writes_skip_the_length() {
        let (mut v, store) = filled(10);
        let _ = v.set(3, &30).unwrap();
        assert_eq!(store.calls(), 2, "set: a read and a write");
        store.reset_calls();
        let _ = v.update(3, |x| x + 1).unwrap();
        assert_eq!(store.calls(), 2, "update: a read and a write");
        store.reset_calls();
        v.swap(0, 9).unwrap();
        assert_eq!(store.calls(), 4, "swap: two reads and two writes");
        store.reset_calls();
        assert_eq!(v.swap_remove(0).unwrap(), 9);
        assert_eq!(store.calls(), 6, "swap_remove: three reads, three writes");
        assert_eq!(
            v.set(9, &0).unwrap_err(),
            StorageError::OutOfBounds { index: 9, len: 9 }
        );
    }

    #[test]
    fn shifting_reads_the_tail_through_one_cursor() {
        let (mut v, store) = filled(2_500);
        v.insert(0, &9_999).unwrap();
        // Length, open, four batches, 2,500 moves, the new value, the length add.
        assert_eq!(store.calls(), 1 + 1 + 4 + 2_500 + 1 + 1);
        store.reset_calls();
        assert_eq!(v.remove(0).unwrap(), 9_999);
        // Length, open, four batches (2,501 entries), 2,500 moves, a delete, the add.
        assert_eq!(store.calls(), 1 + 1 + 4 + 2_500 + 1 + 1);
        assert_eq!(all(v.iter()), (0..2_500).collect::<Vec<_>>());
    }

    #[test]
    fn retain_reads_once_and_writes_only_what_moves() {
        let (mut v, store) = filled(2_000);
        let writes = store.writes();
        assert_eq!(v.retain(|x| *x >= 1_000).unwrap(), 1_000);
        // 1,000 kept elements move down, 1,000 positions are deleted, one add.
        assert_eq!(store.writes() - writes, 2_001);
        assert_eq!(
            store.calls(),
            1 + 1 + 3 + 2_001,
            "length, open, three batches, writes"
        );
        assert_eq!(v.first().unwrap(), Some(1_000));
    }

    #[test]
    fn a_map_counts_its_new_keys_with_one_add() {
        let store = MemStore::new();
        let mut m: UnorderedMap<u32, u32, MemStore> = UnorderedMap::with_store("m", store.clone());
        let keys: Vec<u32> = (0..2_300).collect();
        assert_eq!(m.extend(keys.iter().map(|k| (k, k))).unwrap(), 2_300);
        assert_eq!(store.writes(), 2_301, "a put per pair and one add");
        store.reset_calls();
        let writes = store.writes();
        assert_eq!(m.retain(|k, _| k % 10 == 0).unwrap(), 2_070);
        assert_eq!(
            store.writes() - writes,
            2_071,
            "a delete per removed key and one add"
        );
        // Open, batches of 1,000, 1,000 and 300, the empty batch, then the writes.
        assert_eq!(store.calls(), 5 + 2_071);
        assert_eq!(m.len().unwrap(), 230);
    }

    #[test]
    fn a_set_counts_its_new_elements_with_one_add() {
        let store = MemStore::new();
        let mut s: Set<u32, MemStore> = Set::with_store("s", store.clone());
        let values: Vec<u32> = (0..1_500).collect();
        assert_eq!(s.extend(&values).unwrap(), 1_500);
        assert_eq!(store.writes(), 1_501);
        let writes = store.writes();
        assert_eq!(s.retain(|v| *v < 500).unwrap(), 1_000);
        assert_eq!(store.writes() - writes, 1_001);
        assert_eq!(s.len().unwrap(), 500);
    }
}
