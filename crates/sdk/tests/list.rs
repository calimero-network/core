//! `List`: an ordered list where inserting and moving anywhere is a few writes, with ids that
//! survive moves.

use calimero_sdk::collections::{List, UnorderedMap};
use calimero_sdk::env::mock;
use calimero_sdk::{MemStore, StorageError};

fn values(list: &List<String, MemStore>) -> Vec<String> {
    list.iter()
        .map(|e| e.map(|(_, v)| v))
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn inserts_and_moves_keep_order_and_ids() {
    mock::reset();
    let mut l: List<String, MemStore> = List::with_store("todo", MemStore::new());
    let b = l.push_back(&"b".into()).unwrap();
    let d = l.push_back(&"d".into()).unwrap();
    let a = l.push_front(&"a".into()).unwrap();
    let c = l.insert_after(&b, &"c".into()).unwrap();
    assert_eq!(values(&l), ["a", "b", "c", "d"]);
    assert_eq!(l.len().unwrap(), 4);
    l.move_to_back(&a).unwrap();
    l.move_before(&d, &b).unwrap();
    assert_eq!(values(&l), ["d", "b", "c", "a"]);
    l.move_after(&d, &c).unwrap();
    l.move_to_front(&c).unwrap();
    assert_eq!(values(&l), ["c", "b", "d", "a"]);
    // Ids survive moves.
    assert_eq!(l.get(&d).unwrap().as_deref(), Some("d"));
    assert_eq!(l.set(&d, &"D".into()).unwrap(), "d");
    assert_eq!(l.remove(&b).unwrap().as_deref(), Some("b"));
    assert_eq!(values(&l), ["c", "D", "a"]);
    assert_eq!(l.first().unwrap().map(|(id, _)| id), Some(c.clone()));
    assert_eq!(l.last().unwrap().map(|(_, v)| v).as_deref(), Some("a"));
    assert_eq!(l.get(&b).unwrap(), None);
    assert!(matches!(
        l.insert_after(&b, &"x".into()),
        Err(StorageError::UnknownItem(_))
    ));
    // Moving next to itself or where it already is changes nothing.
    l.move_after(&c, &c).unwrap();
    l.move_before(&c, &d).unwrap();
    assert_eq!(values(&l), ["c", "D", "a"]);
    mock::reset();
}

#[test]
fn inserting_and_moving_cost_the_same_at_the_front_of_ten_thousand() {
    mock::reset();
    let store = MemStore::new();
    let mut l: List<u32, MemStore> = List::with_store("big", store.clone());
    let mut ids = Vec::new();
    for i in 0..10_000 {
        ids.push(l.push_back(&i).unwrap());
    }
    store.reset_calls();
    let writes = store.writes();
    let _ = l.insert_before(&ids[0], &99).unwrap();
    // The anchor's position, its neighbour (open + one entry), two puts, the length.
    assert_eq!(store.calls(), 1 + 2 + 3);
    assert_eq!(store.writes() - writes, 3, "a Vector would rewrite 10,000");
    store.reset_calls();
    let writes = store.writes();
    l.move_after(&ids[9_999], &ids[0]).unwrap();
    assert_eq!(
        store.writes() - writes,
        3,
        "a move is delete, put, re-point"
    );
    assert!(store.calls() <= 8);
    let order: Vec<u32> = l
        .iter()
        .take(3)
        .map(|e| e.map(|(_, v)| v))
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(order, [99, 0, 9_999]);
    mock::reset();
}

#[test]
fn a_thousand_inserts_into_one_gap_stay_in_order() {
    mock::reset();
    let mut l: List<u32, MemStore> = List::with_store("gap", MemStore::new());
    let first = l.push_back(&0).unwrap();
    let _last = l.push_back(&1_000_000).unwrap();
    // Each new item goes right after the first: the newest comes second.
    for i in 1..=1_000 {
        let _ = l.insert_after(&first, &i).unwrap();
    }
    let got: Vec<u32> = l
        .iter()
        .map(|e| e.map(|(_, v)| v))
        .collect::<Result<_, _>>()
        .unwrap();
    let mut want = vec![0];
    want.extend((1..=1_000).rev());
    want.push(1_000_000);
    assert_eq!(got, want);
    mock::reset();
}

#[test]
fn a_list_per_key_and_clearing_a_slice_at_a_time() {
    mock::reset();
    let store = MemStore::new();
    let boards: UnorderedMap<String, List<String, MemStore>, MemStore> =
        UnorderedMap::with_store("boards", store.clone());
    let mut todo = boards.get(&"todo".into()).unwrap();
    let mut done = boards.get(&"done".into()).unwrap();
    for t in ["a", "b", "c"] {
        let _ = todo.push_back(&t.into()).unwrap();
    }
    let _ = done.push_back(&"z".into()).unwrap();
    assert_eq!(todo.len().unwrap(), 3);
    assert_eq!(values(&boards.get(&"done".into()).unwrap()), ["z"]);
    let p = todo.clear_some(2).unwrap();
    assert_eq!((p.removed, p.done), (2, false));
    assert_eq!(values(&todo), ["c"]);
    assert!(todo.clear_some(10).unwrap().done);
    assert!(todo.is_empty().unwrap());
    assert!(
        !boards.contains(&"todo".into()).unwrap(),
        "an emptied list no longer counts"
    );
    mock::reset();
}
