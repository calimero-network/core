//! `Table::len`: a maintained length, exact after every write that adds or removes records,
//! one read to answer, one more write per insert or removal, and honest about tables stored
//! before it existed.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use calimero_sdk::collections::UnorderedMap;
use calimero_sdk::encoding::key;
use calimero_sdk::prelude::*;
use calimero_sdk::{MemStore, Progress, StorageError, Store, TableError};

#[derive(Record, Clone, Debug, PartialEq)]
#[table(
    key = "id",
    unique(email = ["email"]),
    index(by_team = ["team"]),
    count(per_team = ["team"])
)]
pub struct Member {
    pub id: u32,
    pub email: String,
    pub team: String,
}

fn member(id: u32, team: &str) -> Member {
    Member {
        id,
        email: format!("m{id}@x"),
        team: team.into(),
    }
}

fn table() -> (Table<Member, MemStore>, MemStore) {
    let store = MemStore::new();
    (Table::with_store("members", store.clone()), store)
}

fn fill(t: &mut Table<Member, MemStore>, ids: core::ops::Range<u32>) {
    for id in ids {
        t.insert(member(id, if id % 2 == 0 { "red" } else { "blue" }))
            .unwrap();
    }
}

/// The length, checked against the records themselves.
fn len(t: &Table<Member, MemStore>) -> u64 {
    let n = t.len().unwrap();
    assert_eq!(n, t.iter().count() as u64, "len agrees with the records");
    assert_eq!(t.is_empty().unwrap(), n == 0);
    n
}

fn row(kind: &str) -> Vec<u8> {
    key::encode(&[KeyPart::Str("members".into()), KeyPart::Str(kind.into())])
}

/// What a table written before the length existed looks like: its rows without `n` and `l`.
fn forget_len(store: &MemStore) {
    let mut store = store.clone();
    store.delete(&row("n"));
    store.delete(&row("l"));
}

#[test]
fn len_is_exact_across_every_write() {
    let (mut t, _) = table();
    assert_eq!(len(&t), 0);
    fill(&mut t, 0..10);
    assert_eq!(len(&t), 10);

    // Refused writes change nothing: an existing key, a unique clash, a changed key.
    assert!(matches!(
        t.insert(member(3, "red")),
        Err(TableError::Duplicate { .. })
    ));
    let clash = Member {
        id: 99,
        email: "m3@x".into(),
        team: "red".into(),
    };
    assert!(matches!(
        t.insert(clash.clone()),
        Err(TableError::Duplicate { .. })
    ));
    assert!(matches!(t.upsert(clash), Err(TableError::Duplicate { .. })));
    assert!(t.update(&3u32, |m| m.id = 100).is_err());
    assert!(t.update(&3u32, |m| m.email = "m4@x".into()).is_err());
    assert_eq!(len(&t), 10);

    // Updates and upserts of an existing key move nothing; an upsert of a new key adds one.
    assert!(t.update(&3u32, |m| m.team = "red".into()).unwrap());
    assert!(!t.update(&50u32, |m| m.team = "red".into()).unwrap());
    t.upsert(member(4, "blue")).unwrap();
    assert_eq!(len(&t), 10);
    t.upsert(member(10, "red")).unwrap();
    assert_eq!(len(&t), 11);

    // Removing a present key takes one; an absent one nothing.
    assert!(t.remove(&10u32).unwrap());
    assert!(!t.remove(&10u32).unwrap());
    assert_eq!(len(&t), 10);

    // retain: 0..10 holds ids 0, 2, 4, 6, 8 on red, then 3 and 4 swapped teams above.
    assert_eq!(t.retain(|m| m.id != 0).unwrap(), 1);
    assert_eq!(len(&t), 9);
    assert_eq!(t.retain(|_| true).unwrap(), 0);
    assert_eq!(len(&t), 9);

    // remove_by: a slice, then the rest.
    let red = [KeyPart::Str("red".into())];
    let blue_before = t.count("perTeam", &[KeyPart::Str("blue".into())]).unwrap();
    let red_before = t.count("perTeam", &red).unwrap();
    assert_eq!(
        t.remove_by("byTeam", &red, 2).unwrap(),
        Progress {
            removed: 2,
            done: false
        }
    );
    assert_eq!(len(&t), 9 - 2);
    let rest = t.remove_by("byTeam", &red, u64::MAX).unwrap();
    assert!(rest.done);
    assert_eq!(rest.removed, red_before - 2);
    assert_eq!(len(&t), blue_before);
    // A refused remove_by changes nothing.
    assert!(matches!(
        t.remove_by("nope", &[], 1),
        Err(StorageError::UnknownName(_))
    ));
    assert_eq!(len(&t), blue_before);

    // clear_some a slice at a time, then refill and clear.
    let first = t.clear_some(1).unwrap();
    assert_eq!(first.removed, 1);
    assert_eq!(len(&t), blue_before - 1);
    assert!(t.clear_some(u64::MAX).unwrap().done);
    assert_eq!(len(&t), 0);
    fill(&mut t, 0..5);
    assert_eq!(len(&t), 5);
    assert_eq!(t.clear().unwrap(), 5);
    assert_eq!(len(&t), 0);
    // After clear the table starts its length again on the next insert.
    fill(&mut t, 0..3);
    assert_eq!(len(&t), 3);
}

#[test]
fn a_table_per_key_keeps_its_own_length() {
    let store = MemStore::new();
    let mut by_org: UnorderedMap<String, Table<Member, MemStore>, MemStore> =
        UnorderedMap::with_store("orgs", store.clone());
    let mut acme = by_org.get(&"acme".into()).unwrap();
    let mut other = by_org.get(&"other".into()).unwrap();
    fill(&mut acme, 0..4);
    fill(&mut other, 0..2);
    assert_eq!(len(&acme), 4);
    assert_eq!(len(&other), 2);
    // Emptied by removals, a nested table no longer counts as holding anything: its length
    // at 0 and its mark are bookkeeping.
    assert_eq!(other.retain(|_| false).unwrap(), 2);
    assert_eq!(len(&other), 0);
    assert!(!by_org.contains(&"other".into()).unwrap());
    assert_eq!(by_org.keys().count(), 1);
    // Removing a key's table removes its length with it.
    assert!(by_org.remove(&"acme".into()).unwrap() > 0);
    assert_eq!(len(&by_org.get(&"acme".into()).unwrap()), 0);
    assert!(by_org.is_empty().unwrap());
}

mod costs {
    use super::*;

    /// The host calls and writes of `f`.
    fn cost(store: &MemStore, f: impl FnOnce()) -> (usize, usize) {
        store.reset_costs();
        let writes = store.writes();
        f();
        (store.calls(), store.writes() - writes)
    }

    #[test]
    fn len_is_one_read() {
        let (mut t, store) = table();
        fill(&mut t, 0..2_500);
        assert_eq!(cost(&store, || assert_eq!(t.len().unwrap(), 2_500)), (1, 0));
        assert_eq!(cost(&store, || assert!(!t.is_empty().unwrap())), (1, 0));
    }

    // Before the length, with one unique, one index and one count: an insert was 6 calls and
    // 4 writes (two gets, the record, the index and unique entries, the count's add), an
    // update 6 and 5 here (the index and the count move), a removal 5 and 4. The numbers
    // below are those plus the length: one get of its mark and one `add`.
    #[test]
    fn an_insert_or_removal_costs_one_write_more_and_an_update_none() {
        let (mut t, store) = table();
        fill(&mut t, 0..2);

        assert_eq!(
            cost(&store, || t.insert(member(2, "red")).unwrap()),
            (6 + 2, 4 + 1)
        );
        assert_eq!(
            cost(&store, || assert!(t
                .update(&2u32, |m| m.team = "blue".into())
                .unwrap())),
            (6, 5)
        );
        assert_eq!(
            cost(&store, || t.upsert(member(2, "red")).unwrap()),
            (6, 5),
            "an upsert of an existing key is an update"
        );
        assert_eq!(
            cost(&store, || t.upsert(member(3, "red")).unwrap()),
            (6 + 2, 4 + 1)
        );
        assert_eq!(
            cost(&store, || assert!(t.remove(&3u32).unwrap())),
            (5 + 2, 4 + 1)
        );
        // Refusals and misses read what they did before and write nothing.
        assert_eq!(cost(&store, || assert!(!t.remove(&3u32).unwrap())), (1, 0));
        assert_eq!(
            cost(&store, || assert!(t.insert(member(2, "red")).is_err())),
            (1, 0)
        );
        let clash = Member {
            id: 7,
            email: "m2@x".into(),
            team: "red".into(),
        };
        assert_eq!(cost(&store, || assert!(t.insert(clash).is_err())), (2, 0));
        assert_eq!(len(&t), 3);
    }

    #[test]
    fn bulk_removals_move_the_length_once() {
        let (mut t, store) = table();
        fill(&mut t, 0..100);
        // Before: open, one batch of 100, the empty batch, then 4 writes per removed record.
        assert_eq!(
            cost(&store, || assert_eq!(
                t.retain(|m| m.team == "red").unwrap(),
                50
            )),
            (3 + 50 * 4 + 2, 50 * 4 + 1)
        );
        assert_eq!(
            cost(&store, || assert_eq!(t.retain(|_| true).unwrap(), 0)),
            (3, 0),
            "nothing removed, the length untouched"
        );
        let (_, writes) = cost(&store, || {
            assert_eq!(t.clear_some(10).unwrap().removed, 10);
        });
        assert_eq!(writes, 10 * 4 + 1);
        let (_, writes) = cost(&store, || {
            assert_eq!(
                t.remove_by("byTeam", &[KeyPart::Str("red".into())], 10)
                    .unwrap()
                    .removed,
                10
            );
        });
        assert_eq!(writes, 10 * 4 + 1);
        assert_eq!(len(&t), 30);
    }

    #[test]
    fn the_length_is_written_once_and_then_only_added_to() {
        let (mut t, store) = table();
        // A fresh table's first insert finds no length: it counts the records (an open, the
        // batch with the new one, the empty batch) and writes the length and its mark, two
        // writes more than before.
        assert_eq!(
            cost(&store, || t.insert(member(0, "red")).unwrap()),
            (6 + 1 + 3 + 2, 4 + 2)
        );
        fill(&mut t, 1..5);
        assert!(t.remove(&1u32).unwrap());
        assert_eq!(store.op_counts(&row("n")), (1, 5));
        assert_eq!(store.op_counts(&row("l")), (1, 0));
        assert_eq!(len(&t), 4);
    }
}

mod tables_without_a_length {
    use super::*;

    #[test]
    fn len_counts_the_records_until_a_write_keeps_it() {
        let (mut t, store) = table();
        fill(&mut t, 0..2_500);
        forget_len(&store);
        assert!(store.get(&row("n")).is_none());

        // Counted, honestly: a get, an open, three batches of 1,000 and the empty one.
        store.reset_costs();
        assert_eq!(t.len().unwrap(), 2_500);
        assert_eq!(store.calls(), 1 + 1 + 3 + 1);
        assert!(!t.is_empty().unwrap());

        // The first insert counts what is there and writes the length; from then on it is
        // one read.
        t.insert(member(2_500, "red")).unwrap();
        assert_eq!(store.get(&row("n")), Some(vec![0x19, 0x09, 0xc5]));
        store.reset_costs();
        assert_eq!(t.len().unwrap(), 2_501);
        assert_eq!(store.calls(), 1);
        assert_eq!(len(&t), 2_501);
    }

    #[test]
    fn a_removal_first_starts_the_length_from_what_is_left() {
        let (mut t, store) = table();
        fill(&mut t, 0..10);
        forget_len(&store);
        assert!(t.remove(&3u32).unwrap());
        assert_eq!(len(&t), 9);
        assert!(!t.remove(&3u32).unwrap());
        assert_eq!(len(&t), 9);
    }

    #[test]
    fn bulk_removals_start_the_length_from_what_is_left() {
        let (mut t, store) = table();
        fill(&mut t, 0..10);
        forget_len(&store);
        assert_eq!(t.retain(|m| m.id < 6).unwrap(), 4);
        assert_eq!(len(&t), 6);

        forget_len(&store);
        assert_eq!(t.clear_some(2).unwrap().removed, 2);
        assert_eq!(len(&t), 4);

        forget_len(&store);
        let blue = [KeyPart::Str("blue".into())];
        let removed = t.remove_by("byTeam", &blue, u64::MAX).unwrap().removed;
        assert_eq!(len(&t), 4 - removed);
    }

    #[test]
    fn writes_that_move_nothing_leave_it_unkept() {
        let (mut t, store) = table();
        fill(&mut t, 0..4);
        forget_len(&store);
        assert!(t.update(&0u32, |m| m.team = "blue".into()).unwrap());
        t.upsert(member(1, "red")).unwrap();
        assert!(t.insert(member(1, "red")).is_err());
        assert_eq!(t.retain(|_| true).unwrap(), 0);
        assert!(store.get(&row("n")).is_none());
        assert_eq!(len(&t), 4);
    }
}
