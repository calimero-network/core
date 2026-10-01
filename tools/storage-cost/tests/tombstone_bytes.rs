//! The gate on tombstone GC: once it has run past the retention, a deleted
//! entry costs no byte, neither as its own row nor in its map's. Row counters
//! cannot see the second: the map is one row however many ids it lists, and
//! every write to the map rewrites the list.

use calimero_storage::collections::{Root, UnorderedMap};
use calimero_storage::store::MainStorage;
use storage_cost::{collect_garbage, measure, reset_counters, resident};

type Map = UnorderedMap<String, String, MainStorage>;

const FIELD: &str = "tombstones";
const FEW: usize = 10; // entries deleted before the measured write
const MANY: usize = 10_000; // a thousand times as many, one identical write
const MAX_GROWTH: f64 = 1.5; // trie-shape headroom; listing MANY ids costs 300x
/// The one byte live state legitimately gains: the map's child trie keeps a
/// varint of every insert it has ever ordered (`next_order`), which a thousand
/// inserts take from one byte to two.
const NEXT_ORDER_GROWTH: u64 = 1;

/// Builds a map holding one live entry, inserts `n` more and deletes them,
/// each phase its own commit, as separate calls are.
fn delete_n(n: usize) {
    let mut map = Root::new(|| Map::new_with_field_name(FIELD));
    drop(map.insert("kept".to_owned(), "kept".to_owned()));
    map.commit();
    let mut map = Root::<Map>::fetch().expect("map should exist");
    for i in 0..n {
        drop(map.insert(format!("key-{i}"), format!("value-{i}")));
    }
    map.commit();
    let mut map = Root::<Map>::fetch().expect("map should exist");
    for i in 0..n {
        drop(map.remove(&format!("key-{i}")));
    }
    map.commit();
}

#[test]
fn deleted_entries_hold_no_bytes_once_collected() {
    let ((built, deleted, collected), _) = measure(|| {
        let mut map = Root::new(|| Map::new_with_field_name(FIELD));
        drop(map.insert("kept".to_owned(), "kept".to_owned()));
        map.commit();
        let built = resident();
        let mut map = Root::<Map>::fetch().expect("map should exist");
        for i in 0..1_000 {
            drop(map.insert(format!("key-{i}"), format!("value-{i}")));
        }
        map.commit();
        let mut map = Root::<Map>::fetch().expect("map should exist");
        for i in 0..1_000 {
            drop(map.remove(&format!("key-{i}")));
        }
        map.commit();
        let deleted = resident();
        collect_garbage();
        (built, deleted, resident())
    });
    assert!(
        deleted.1 > built.1,
        "deleting left nothing to collect ({built:?} -> {deleted:?}): the workload measures nothing"
    );
    assert!(
        collected.0 == built.0 && collected.1 <= built.1 + NEXT_ORDER_GROWTH,
        "1000 entries inserted and deleted still hold rows or bytes after GC \
         ({built:?} before, {deleted:?} deleted, {collected:?} collected)"
    );
}

fn insert_bytes_after(n: usize) -> u64 {
    let (_, costs) = measure(|| {
        delete_n(n);
        collect_garbage();
        reset_counters();
        let mut map = Root::<Map>::fetch().expect("map should exist");
        drop(map.insert("new".to_owned(), "new".to_owned()));
        map.commit();
    });
    costs.bytes_written
}

#[ignore = "slow: inserts and deletes 10,000 entries; run by the release storage-cost CI job via --include-ignored"]
#[test]
fn a_write_after_deletes_does_not_carry_them() {
    let few = insert_bytes_after(FEW);
    let many = insert_bytes_after(MANY);
    assert!(
        many as f64 <= few as f64 * MAX_GROWTH,
        "one insert wrote {few} bytes after {FEW} collected deletes and {many} after \
         {MANY} - the map still lists ids whose tombstones GC has collected"
    );
}
