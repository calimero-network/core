//! Removing many records or entries: once, without a second read per record, and a slice at
//! a time when one run's 10,000 writes are not enough.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use calimero_sdk::collections::{Set, UnorderedMap, Vector};
use calimero_sdk::prelude::*;
use calimero_sdk::{MemStore, Progress, StorageError};

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

fn members(n: u32) -> (Table<Member, MemStore>, MemStore) {
    let store = MemStore::new();
    let mut t = Table::with_store("members", store.clone());
    for id in 0..n {
        t.insert(Member {
            id,
            email: format!("m{id}@x"),
            team: if id % 2 == 0 { "red" } else { "blue" }.into(),
        })
        .unwrap();
    }
    (t, store)
}

fn team(t: &Table<Member, MemStore>, name: &str) -> u64 {
    t.count("perTeam", &[name.to_key_part()]).unwrap()
}

#[test]
fn retain_reads_each_record_once_and_keeps_indexes_right() {
    let (mut t, store) = members(100);
    store.reset_calls();
    assert_eq!(t.retain(|m| m.team == "red").unwrap(), 50);
    // Open, one batch of 100, the empty batch; then per removed record its record, index
    // and unique deletes and its count add, no get of a record; then the length once (a get
    // of its mark and one add).
    assert_eq!(store.calls(), 3 + 50 * 4 + 2);
    assert_eq!(team(&t, "blue"), 0);
    assert_eq!(team(&t, "red"), 50);
    assert_eq!(
        t.find_iter("byTeam", &["blue".to_key_part()], FindOpts::default())
            .count(),
        0
    );
    // The removed emails are free again.
    t.insert(Member {
        id: 1,
        email: "m1@x".into(),
        team: "red".into(),
    })
    .unwrap();
}

#[test]
fn remove_by_takes_what_an_index_finds_a_slice_at_a_time() {
    let (mut t, _) = members(100);
    let first = t.remove_by("byTeam", &["blue".to_key_part()], 30).unwrap();
    assert_eq!(
        first,
        Progress {
            removed: 30,
            done: false
        }
    );
    let rest = t
        .remove_by("byTeam", &["blue".to_key_part()], u64::MAX)
        .unwrap();
    assert_eq!(
        rest,
        Progress {
            removed: 20,
            done: true
        }
    );
    assert_eq!(team(&t, "blue"), 0);
    assert_eq!(team(&t, "red"), 50);
    assert!(matches!(
        t.remove_by("nope", &[], 1),
        Err(StorageError::UnknownName(_))
    ));
}

#[test]
fn clear_some_stays_under_the_write_limit_and_finishes() {
    // 3,000 records at 4 writes each is 12,000: past one run's 10,000.
    let (mut t, store) = members(3_000);
    let mut runs = 0;
    loop {
        let writes = store.writes();
        let p = t.clear_some(2_000).unwrap();
        runs += 1;
        assert!(store.writes() - writes <= 10_000, "one slice fits in a run");
        if p.done {
            break;
        }
    }
    assert_eq!(runs, 2);
    assert_eq!(t.iter().count(), 0);
    assert_eq!(team(&t, "red"), 0);
}

#[test]
fn maps_sets_and_vectors_clear_a_slice_at_a_time() {
    let store = MemStore::new();
    let mut m: UnorderedMap<u32, u32, MemStore> = UnorderedMap::with_store("m", store.clone());
    let keys: Vec<u32> = (0..12_000).collect();
    let _ = m.extend(keys.iter().map(|k| (k, k))).unwrap();
    assert_eq!(
        m.clear_some(10_000).unwrap(),
        Progress {
            removed: 10_000,
            done: false
        }
    );
    assert_eq!(m.len().unwrap(), 2_000);
    assert!(m.clear_some(10_000).unwrap().done);
    assert!(m.is_empty().unwrap());

    let mut s: Set<u32, MemStore> = Set::with_store("s", store.clone());
    let _ = s.extend(&keys[..50]).unwrap();
    assert_eq!(
        s.clear_some(50).unwrap(),
        Progress {
            removed: 50,
            done: true
        }
    );
    assert_eq!(s.len().unwrap(), 0);

    let mut v: Vector<u32, MemStore> = Vector::with_store("v", store);
    let _ = v.extend(&keys[..10]).unwrap();
    assert_eq!(
        v.clear_some(4).unwrap(),
        Progress {
            removed: 4,
            done: false
        }
    );
    let left: Vec<u32> = v.iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(left, (0..6).collect::<Vec<_>>(), "removed from the end");
    assert!(v.clear_some(100).unwrap().done);
}

#[test]
fn an_app_can_remove_a_departed_members_value() {
    use calimero_sdk::collections::PerUser;
    use calimero_sdk::env::{mock, Caller};
    mock::reset();
    let mut prefs: PerUser<String, MemStore> = PerUser::with_store("prefs", MemStore::new());
    mock::with_mut(|s| s.caller = Caller::Account("bo".into()));
    let _ = prefs.set(&"dark".into()).unwrap();
    mock::with_mut(|s| s.caller = Caller::Account("admin".into()));
    assert_eq!(prefs.remove_for("bo").unwrap().as_deref(), Some("dark"));
    assert!(!prefs.contains_for("bo").unwrap());
    mock::reset();
}

#[test]
fn an_update_that_keeps_its_unique_values_does_not_touch_them() {
    let (mut t, store) = members(3);
    store.reset_calls();
    // Moving a member to another team: the email stays, so its unique entry is not read or
    // written again; the index entry and the counts move.
    assert!(t.update(&0u32, |m| m.team = "blue".into()).unwrap());
    // Read the record; write it; move the index entry (delete, put); move the count (two adds).
    assert_eq!(store.calls(), 1 + 1 + 2 + 2);
    assert_eq!(team(&t, "blue"), 2);
}

#[test]
fn first_and_last_read_one_entry() {
    let store = MemStore::new();
    let mut m: UnorderedMap<u32, u32, MemStore> = UnorderedMap::with_store("m", store.clone());
    let keys: Vec<u32> = (0..10).collect();
    let _ = m.extend(keys.iter().map(|k| (k, k))).unwrap();
    store.reset_pulled();
    assert_eq!(m.first().unwrap(), Some((0, 0)));
    assert_eq!(m.last().unwrap(), Some((9, 9)));
    assert_eq!(store.pulled(), 2, "one entry each");
}
