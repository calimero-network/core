use borsh::{BorshDeserialize, BorshSerialize};

use super::{IndexValue, Indexed, IndexedMap};
use crate::collections::{LwwRegister, UnorderedMap};
use crate::entities::Data;
use crate::env;
use crate::store::mocked::{self, MockedStorage};
use crate::store::{Key, StorageAdaptor};

/// Backs the ordered index.
type Backed = MockedStorage<955>;

/// An issue as an app would store it: indexed by status, assignee, each label,
/// and `(status, created_at)` for "the newest issues in one status".
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq)]
struct Issue {
    status: LwwRegister<String>,
    assignee: LwwRegister<Option<String>>,
    labels: LwwRegister<Vec<String>>,
    created_at: LwwRegister<u64>,
}

impl Indexed for Issue {
    const INDEXES: &'static [&'static str] = &["status", "assignee", "labels", "status_created"];

    fn index_keys(&self, index: usize, out: &mut Vec<Vec<u8>>) {
        match index {
            0 => self.status.encode_index(out),
            1 => self.assignee.encode_index(out),
            2 => self.labels.encode_index(out),
            3 => (&self.status, &self.created_at).encode_index(out),
            _ => {}
        }
    }
}

fn issue(status: &str, assignee: Option<&str>, labels: &[&str], created_at: u64) -> Issue {
    Issue {
        status: LwwRegister::new(status.to_owned()),
        assignee: LwwRegister::new(assignee.map(str::to_owned)),
        labels: LwwRegister::new(labels.iter().map(|l| (*l).to_owned()).collect()),
        created_at: LwwRegister::new(created_at),
    }
}

fn tracker() -> IndexedMap<String, Issue, Backed> {
    env::reset_for_testing();
    let mut map = IndexedMap::new();
    for (id, value) in [
        ("i1", issue("open", Some("alice"), &["bug", "ui"], 10)),
        ("i2", issue("closed", Some("bob"), &["bug"], 20)),
        ("i3", issue("open", None, &["feature"], 30)),
        ("i4", issue("open", Some("alice"), &[], 40)),
        ("i5", issue("review", Some("carol"), &["ui"], 50)),
    ] {
        let _ = map.insert(id.to_owned(), value).unwrap();
    }
    map
}

fn sorted(mut keys: Vec<String>) -> Vec<String> {
    keys.sort();
    keys
}

#[test]
fn eq_selects_exactly_the_matching_entries() {
    let map = tracker();

    assert_eq!(
        sorted(map.query("status").eq("open").keys().unwrap()),
        ["i1", "i3", "i4"]
    );
    assert_eq!(map.query("status").eq("closed").keys().unwrap(), ["i2"]);
    assert!(map
        .query("status")
        .eq("nobody-uses-this")
        .keys()
        .unwrap()
        .is_empty());
    assert_eq!(map.query("status").eq("open").count().unwrap(), 3);
}

#[test]
fn a_value_is_never_matched_by_one_it_is_a_prefix_of() {
    env::reset_for_testing();
    let mut map: IndexedMap<String, Issue, Backed> = IndexedMap::new();
    for (id, status) in [("a", "ab"), ("b", "abc"), ("c", "a"), ("d", "ab\0")] {
        let _ = map
            .insert(id.to_owned(), issue(status, None, &[], 0))
            .unwrap();
    }

    assert_eq!(map.query("status").eq("ab").keys().unwrap(), ["a"]);
    assert_eq!(map.query("status").eq("a").keys().unwrap(), ["c"]);
    // An embedded zero byte is escaped, not a terminator.
    assert_eq!(map.query("status").eq("ab\0").keys().unwrap(), ["d"]);
}

#[test]
fn none_leaves_an_entry_out_and_a_list_puts_it_in_once_per_element() {
    let map = tracker();

    assert_eq!(
        sorted(map.query("assignee").eq(&Some("alice")).keys().unwrap()),
        ["i1", "i4"]
    );
    // i3 has no assignee, so the assignee index holds four rows, not five.
    assert_eq!(map.query("assignee").count().unwrap(), 4);

    assert_eq!(
        sorted(map.query("labels").eq("bug").keys().unwrap()),
        ["i1", "i2"]
    );
    assert_eq!(
        sorted(map.query("labels").eq("ui").keys().unwrap()),
        ["i1", "i5"]
    );
    // One row per label: i1 twice, i2, i3, i5 — i4 has none.
    assert_eq!(map.query("labels").count().unwrap(), 5);
}

#[test]
fn a_compound_index_orders_by_its_free_component() {
    let map = tracker();

    // "open, oldest first" and "open, newest first" are one seek each.
    assert_eq!(
        map.query("status_created").eq("open").keys().unwrap(),
        ["i1", "i3", "i4"]
    );
    assert_eq!(
        map.query("status_created")
            .eq("open")
            .desc()
            .keys()
            .unwrap(),
        ["i4", "i3", "i1"]
    );
    assert_eq!(
        map.query("status_created")
            .eq("open")
            .desc()
            .limit(2)
            .keys()
            .unwrap(),
        ["i4", "i3"]
    );
    assert_eq!(
        map.query("status_created")
            .eq("open")
            .desc()
            .skip(1)
            .limit(1)
            .keys()
            .unwrap(),
        ["i3"]
    );
    assert_eq!(
        map.query("status_created")
            .eq("open")
            .eq(&30_u64)
            .keys()
            .unwrap(),
        ["i3"]
    );
    // Every index is also a total order over its rows.
    assert_eq!(
        map.query("status_created").keys().unwrap(),
        ["i2", "i1", "i3", "i4", "i5"]
    );
}

#[test]
fn a_range_bounds_the_component_after_the_pinned_ones() {
    let map = tracker();
    let open_between = |range: (std::ops::Bound<u64>, std::ops::Bound<u64>)| {
        map.query("status_created")
            .eq("open")
            .range(range)
            .keys()
            .unwrap()
    };
    use std::ops::Bound::{Excluded, Included, Unbounded};

    assert_eq!(open_between((Included(10), Included(30))), ["i1", "i3"]);
    assert_eq!(open_between((Excluded(10), Included(30))), ["i3"]);
    assert_eq!(open_between((Included(10), Excluded(30))), ["i1"]);
    assert_eq!(open_between((Excluded(10), Unbounded)), ["i3", "i4"]);
    assert_eq!(open_between((Unbounded, Excluded(40))), ["i1", "i3"]);
    assert!(open_between((Included(11), Excluded(30))).is_empty());

    // The same, through ordinary range syntax and descending.
    assert_eq!(
        map.query("status_created")
            .eq("open")
            .range(20_u64..)
            .desc()
            .keys()
            .unwrap(),
        ["i4", "i3"]
    );
}

#[test]
fn integers_order_by_value_including_negative_ones() {
    #[derive(BorshSerialize, BorshDeserialize)]
    struct Reading {
        delta: i64,
    }
    impl Indexed for Reading {
        const INDEXES: &'static [&'static str] = &["delta"];
        fn index_keys(&self, _index: usize, out: &mut Vec<Vec<u8>>) {
            self.delta.encode_index(out);
        }
    }

    env::reset_for_testing();
    let mut map: IndexedMap<String, Reading, Backed> = IndexedMap::new();
    for (id, delta) in [("a", 5), ("b", -3), ("c", 0), ("d", i64::MIN), ("e", 300)] {
        let _ = map.insert(id.to_owned(), Reading { delta }).unwrap();
    }

    assert_eq!(
        map.query("delta").keys().unwrap(),
        ["d", "b", "c", "a", "e"]
    );
    assert_eq!(
        map.query("delta").range(-3_i64..=5).keys().unwrap(),
        ["b", "c", "a"]
    );
    assert_eq!(
        map.query("delta").desc().first().unwrap().map(|(k, _)| k),
        Some("e".to_owned())
    );
}

#[test]
fn update_moves_an_entry_between_index_values_and_remove_drops_it() {
    let mut map = tracker();

    let moved = map
        .update("i1", |issue| issue.status.set("closed".to_owned()))
        .unwrap();
    assert!(moved.is_some());
    assert_eq!(
        sorted(map.query("status").eq("open").keys().unwrap()),
        ["i3", "i4"]
    );
    assert_eq!(
        sorted(map.query("status").eq("closed").keys().unwrap()),
        ["i1", "i2"]
    );
    // The compound index moved too.
    assert_eq!(
        map.query("status_created").eq("closed").keys().unwrap(),
        ["i1", "i2"]
    );

    assert!(map.update("missing", |_| ()).unwrap().is_none());

    let _ = map.remove("i2").unwrap();
    assert_eq!(map.query("status").eq("closed").keys().unwrap(), ["i1"]);
    assert_eq!(map.query("labels").eq("bug").keys().unwrap(), ["i1"]);

    // Re-inserting a key replaces its rows rather than adding to them.
    let _ = map
        .insert("i1".to_owned(), issue("review", None, &[], 60))
        .unwrap();
    assert!(map.query("status").eq("closed").keys().unwrap().is_empty());
    assert_eq!(
        sorted(map.query("status").eq("review").keys().unwrap()),
        ["i1", "i5"]
    );

    map.clear().unwrap();
    assert_eq!(map.query("status").count().unwrap(), 0);
    assert!(map.is_empty().unwrap());
}

#[test]
fn a_write_that_bypasses_the_indexes_is_caught_and_rebuilt() {
    // What a remote sync looks like from here: the entries change under the
    // map without going through its `insert`/`update`, so its indexes are not
    // told. The collection's hash moves anyway, and the next query rebuilds.
    let mut map = tracker();
    assert_eq!(map.query("status").eq("open").count().unwrap(), 3);

    {
        let mut guard = map.inner.get_mut("i3").unwrap().unwrap();
        guard.status.set("closed".to_owned());
    }
    let _ = map
        .inner
        .insert("i9".to_owned(), issue("open", Some("dave"), &["ops"], 90))
        .unwrap();

    assert_eq!(
        sorted(map.query("status").eq("open").keys().unwrap()),
        ["i1", "i4", "i9"]
    );
    assert_eq!(
        sorted(map.query("status").eq("closed").keys().unwrap()),
        ["i2", "i3"]
    );
    assert_eq!(map.query("labels").eq("ops").keys().unwrap(), ["i9"]);
}

#[test]
fn a_write_after_a_bypass_does_not_certify_the_stale_index() {
    // The marker is only re-stamped by a write that found it current. If this
    // `insert` stamped regardless, the bypassed change to i3 would never be
    // indexed and the query below would still report it open.
    let mut map = tracker();
    assert_eq!(map.query("status").eq("open").count().unwrap(), 3);

    {
        let mut guard = map.inner.get_mut("i3").unwrap().unwrap();
        guard.status.set("closed".to_owned());
    }
    let _ = map
        .insert("i6".to_owned(), issue("open", None, &[], 60))
        .unwrap();

    assert_eq!(
        sorted(map.query("status").eq("open").keys().unwrap()),
        ["i1", "i4", "i6"]
    );
}

#[test]
fn a_rebuild_removes_rows_whose_entries_are_gone() {
    let mut map = tracker();
    assert_eq!(map.query("labels").eq("ui").count().unwrap(), 2);

    let _ = map.inner.remove("i5").unwrap();

    assert_eq!(map.query("labels").eq("ui").keys().unwrap(), ["i1"]);
    assert_eq!(map.query("status").eq("review").count().unwrap(), 0);
}

#[test]
fn an_unordered_map_reads_back_as_an_indexed_map_with_no_migration() {
    // The layout is the UnorderedMap's own: an app switches the field type and
    // its existing data is queryable, the indexes built on the first query.
    env::reset_for_testing();
    let mut plain: UnorderedMap<String, Issue, Backed> = UnorderedMap::new();
    let _ = plain
        .insert("i1".to_owned(), issue("open", None, &[], 1))
        .unwrap();
    let _ = plain
        .insert("i2".to_owned(), issue("closed", None, &[], 2))
        .unwrap();

    let bytes = borsh::to_vec(&plain).unwrap();
    let indexed: IndexedMap<String, Issue, Backed> = borsh::from_slice(&bytes).unwrap();

    assert_eq!(borsh::to_vec(&indexed).unwrap(), bytes);
    assert_eq!(indexed.id(), plain.id());
    assert_eq!(indexed.query("status").eq("open").keys().unwrap(), ["i1"]);
}

#[test]
fn changing_the_declared_indexes_triggers_a_rebuild() {
    // Same stored bytes, a second declaration adding an index: the fingerprint
    // in the marker differs, so the new index is built rather than read empty.
    #[derive(BorshSerialize, BorshDeserialize)]
    struct IssueV2(Issue);
    impl Indexed for IssueV2 {
        const INDEXES: &'static [&'static str] = &["status", "created_at"];
        fn index_keys(&self, index: usize, out: &mut Vec<Vec<u8>>) {
            match index {
                0 => self.0.status.encode_index(out),
                _ => self.0.created_at.encode_index(out),
            }
        }
    }

    let map = tracker();
    assert_eq!(map.query("status").eq("open").count().unwrap(), 3);

    let v2: IndexedMap<String, IssueV2, Backed> =
        borsh::from_slice(&borsh::to_vec(&map).unwrap()).unwrap();
    assert_eq!(
        v2.query("created_at").range(20_u64..=40).keys().unwrap(),
        ["i2", "i3", "i4"]
    );
}

#[test]
fn malformed_queries_fail_when_run_instead_of_answering_wrongly() {
    let map = tracker();

    assert!(map.query("no-such-index").keys().is_err());
    assert!(map.query("status").range("a"..).eq("open").keys().is_err());
    assert!(map
        .query("status")
        .range("a"..)
        .range("b"..)
        .count()
        .is_err());
    // A list encodes to one key per element — it names no single value.
    let two = vec!["bug".to_owned(), "ui".to_owned()];
    assert!(map.query("labels").eq(&two).keys().is_err());
}

#[test]
fn a_query_reads_only_the_rows_it_returns() {
    // The property the collection exists for, as a count rather than a timing:
    // a page from a large index examines the page, not the index.
    env::reset_for_testing();
    let mut map: IndexedMap<String, Issue, Backed> = IndexedMap::new();
    for i in 0..500_u64 {
        let status = if i % 50 == 0 { "open" } else { "closed" };
        let _ = map
            .insert(format!("i{i:03}"), issue(status, None, &[], i))
            .unwrap();
    }
    // Warm the index so the rebuild's own reads are not counted.
    assert_eq!(map.query("status").eq("open").count().unwrap(), 10);

    mocked::reset_index_items();
    let newest = map
        .query("status_created")
        .eq("open")
        .desc()
        .limit(3)
        .keys()
        .unwrap();
    assert_eq!(newest, ["i450", "i400", "i350"]);
    assert_eq!(mocked::index_items_examined(), 3);

    mocked::reset_index_items();
    let oldest = map
        .query("status_created")
        .eq("closed")
        .limit(2)
        .keys()
        .unwrap();
    assert_eq!(oldest, ["i001", "i002"]);
    assert_eq!(mocked::index_items_examined(), 2);
}

/// Storage with no ordered index, like `PrivateStorage`: every query must be
/// answered by the in-memory fallback, identically.
struct Unindexed;

impl StorageAdaptor for Unindexed {
    fn storage_read(key: Key) -> Option<Vec<u8>> {
        MockedStorage::<956>::storage_read(key)
    }

    fn storage_remove(key: Key) -> bool {
        MockedStorage::<956>::storage_remove(key)
    }

    fn storage_write(key: Key, value: &[u8]) -> bool {
        MockedStorage::<956>::storage_write(key, value)
    }
}

#[test]
fn without_an_index_every_query_is_answered_the_same_way_by_scanning() {
    let backed = tracker();
    let mut unindexed: IndexedMap<String, Issue, Unindexed> = IndexedMap::new();
    for (key, value) in backed.entries().unwrap() {
        let _ = unindexed.insert(key, value).unwrap();
    }

    macro_rules! same {
        ($($q:tt)*) => {
            assert_eq!(
                backed.$($q)*.unwrap(),
                unindexed.$($q)*.unwrap(),
                stringify!($($q)*)
            );
        };
    }
    same!(query("status").eq("open").count());
    same!(query("labels").count());
    same!(query("status_created").eq("open").desc().keys());
    same!(query("status_created")
        .eq("open")
        .desc()
        .skip(1)
        .limit(1)
        .keys());
    same!(query("status_created").eq("open").range(10_u64..40).keys());
    same!(query("status_created").keys());
}

/// Storage that claims an index but drops every index write, as an execution
/// with node-local writes suppressed does (the node's migration check).
struct DroppedIndexWrites;

impl StorageAdaptor for DroppedIndexWrites {
    fn storage_read(key: Key) -> Option<Vec<u8>> {
        MockedStorage::<957>::storage_read(key)
    }

    fn storage_remove(key: Key) -> bool {
        MockedStorage::<957>::storage_remove(key)
    }

    fn storage_write(key: Key, value: &[u8]) -> bool {
        MockedStorage::<957>::storage_write(key, value)
    }

    fn index_supported() -> bool {
        true
    }
}

#[test]
fn a_rebuild_whose_writes_are_dropped_answers_by_scanning_instead_of_reading_nothing() {
    let backed = tracker();
    let mut dropped: IndexedMap<String, Issue, DroppedIndexWrites> = IndexedMap::new();
    for (key, value) in backed.entries().unwrap() {
        let _ = dropped.insert(key, value).unwrap();
    }

    assert_eq!(
        sorted(dropped.query("status").eq("open").keys().unwrap()),
        ["i1", "i3", "i4"]
    );
    assert_eq!(
        dropped
            .query("status_created")
            .eq("open")
            .desc()
            .limit(2)
            .keys()
            .unwrap(),
        ["i4", "i3"]
    );
    assert_eq!(dropped.query("labels").count().unwrap(), 5);
}

#[test]
fn a_write_through_the_map_keeps_a_current_index_current() {
    // Maintenance is what spares a query the rebuild: after the first query,
    // every insert, update and remove leaves the marker valid.
    let mut map = tracker();
    assert_eq!(map.query("status").eq("open").count().unwrap(), 3);
    assert!(map.index_current());

    let _ = map
        .insert("i6".to_owned(), issue("open", None, &["ops"], 60))
        .unwrap();
    assert!(map.index_current(), "insert");
    let _ = map
        .update("i6", |issue| issue.status.set("closed".to_owned()))
        .unwrap();
    assert!(map.index_current(), "update");
    let _ = map.remove("i1").unwrap();
    assert!(map.index_current(), "remove");

    assert_eq!(
        sorted(map.query("status").eq("open").keys().unwrap()),
        ["i3", "i4"]
    );
    assert_eq!(map.query("labels").eq("ops").keys().unwrap(), ["i6"]);
}
