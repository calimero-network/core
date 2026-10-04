//! A peer's entry filed under an id its key does not derive, delivered as a delta.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::address::Id;
use crate::collections::{
    compute_collection_id, GCounter, LwwRegister, Mergeable, Root, SortedMap, SortedSet,
    UnorderedMap, UnorderedSet,
};
use crate::delta::StorageDelta;
use crate::entities::{ChildInfo, Data, Metadata};
use crate::env;
use crate::index::Index;
use crate::interface::{Action, ApplyContext, MainInterface};
use crate::logical_clock::HybridTimestamp;
use crate::store::{MainStorage, StorageAdaptor};

/// An entry holding `item`, filed at `filed_at` under `parent`, as a delta carries it.
fn row(parent: Id, filed_at: Id, item: &impl BorshSerialize) -> Action {
    let mut data = borsh::to_vec(item).unwrap();
    data.extend_from_slice(filed_at.as_bytes());
    Action::Add {
        id: filed_at,
        data,
        ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
        metadata: Metadata::new(1, 1),
    }
}

/// `collection` with `rows` synced into it from a peer.
fn synced<T: BorshSerialize + BorshDeserialize>(
    collection: impl FnOnce() -> T,
    rows: impl FnOnce(&mut T) -> Vec<Action>,
) -> Root<T> {
    env::reset_for_testing();
    let mut root = Root::new(collection);
    let actions = rows(&mut root);
    root.commit();
    let delta = StorageDelta::CausalActions {
        actions,
        delta_id: [0; 32],
        delta_hlc: HybridTimestamp::default(),
        effective_writers: Default::default(),
        signer_account: None,
        on_behalf_accounts: Default::default(),
    };
    Root::<T>::sync(&borsh::to_vec(&delta).unwrap(), &ApplyContext::empty()).unwrap();
    Root::<T>::fetch().unwrap()
}

type Map = UnorderedMap<String, u32>;
type Sorted = SortedMap<String, u32>;
type Registers = UnorderedMap<String, LwwRegister<u32>>;

/// "keep" written locally, and a peer's entry holding `key` filed where "other" lives.
fn map_holding(key: &str) -> Root<Map> {
    let map = synced(
        || Map::new_with_field_name("map"),
        |map| {
            map.insert("keep".to_owned(), 1).unwrap();
            vec![row(
                map.id(),
                map.entry_id("other"),
                &(9_u32, key.to_owned()),
            )]
        },
    );
    assert_eq!(map.len().unwrap(), 2, "the misfiled entry must have landed");
    map
}

fn map_with_ghost() -> Root<Map> {
    map_holding("ghost")
}

/// Its ghost sorts after "keep", so an ordered read that includes it ends on it.
fn sorted_map_with_ghost() -> Root<Sorted> {
    let map = synced(
        || Sorted::new_with_field_name("sorted"),
        |map| {
            map.insert("keep".to_owned(), 1).unwrap();
            vec![row(
                map.id(),
                map.entry_id("other"),
                &(9_u32, "zghost".to_owned()),
            )]
        },
    );
    assert_eq!(map.len().unwrap(), 2, "the misfiled entry must have landed");
    map
}

/// "keep" written locally, and a peer's `value` filed where "other" lives.
fn set_holding(value: &str) -> Root<UnorderedSet<String>> {
    let set = synced(
        || UnorderedSet::<String>::new_with_field_name("set"),
        |set| {
            let _ = set.insert("keep".to_owned()).unwrap();
            vec![row(
                compute_collection_id(None, "set"),
                set.entry_id("other"),
                &value.to_owned(),
            )]
        },
    );
    assert_eq!(set.len().unwrap(), 2, "the misfiled value must have landed");
    set
}

/// Its ghost sorts after "keep", so an ordered read that includes it ends on it.
fn sorted_set_with_ghost() -> Root<SortedSet<String>> {
    let set = synced(
        || SortedSet::<String>::new_with_field_name("sorted_set"),
        |set| {
            let _ = set.insert("keep".to_owned()).unwrap();
            vec![row(
                compute_collection_id(None, "sorted_set"),
                set.entry_id("other"),
                &"zghost".to_owned(),
            )]
        },
    );
    assert_eq!(set.len().unwrap(), 2, "the misfiled value must have landed");
    set
}

fn sorted(entries: impl Iterator<Item = (String, u32)>) -> Vec<(String, u32)> {
    let mut entries: Vec<_> = entries.collect();
    entries.sort();
    entries
}

fn listed(set: &UnorderedSet<String>) -> Vec<String> {
    let mut values: Vec<_> = set.iter().unwrap().collect();
    values.sort();
    values
}

#[test]
fn a_map_entry_filed_under_another_keys_id_is_not_listed() {
    let map = map_with_ghost();
    assert_eq!(sorted(map.entries().unwrap()), [("keep".to_owned(), 1)]);
}

#[test]
fn a_misfiled_map_entry_carrying_a_real_key_does_not_shadow_it() {
    let map = map_holding("keep");
    assert_eq!(sorted(map.entries().unwrap()), [("keep".to_owned(), 1)]);
    assert_eq!(map.get("keep").unwrap().map(|value| *value), Some(1));
}

#[test]
fn a_write_at_that_key_replaces_the_misfiled_entry() {
    let mut map = map_with_ghost();
    assert_eq!(map.insert("other".to_owned(), 2).unwrap(), None);
    assert_eq!(
        sorted(map.entries().unwrap()),
        [("keep".to_owned(), 1), ("other".to_owned(), 2)]
    );
}

#[test]
fn a_get_does_not_return_a_misfiled_map_entry() {
    let map = map_with_ghost();
    assert!(map.get("other").unwrap().is_none());
    assert!(map.get("ghost").unwrap().is_none());
}

#[test]
fn a_get_mut_does_not_return_a_misfiled_map_entry() {
    let mut map = map_with_ghost();
    assert!(map.get_mut("other").unwrap().is_none());
}

#[test]
fn a_contains_does_not_see_a_misfiled_map_entry() {
    let map = map_with_ghost();
    assert!(!map.contains("other").unwrap());
}

#[test]
fn a_remove_at_that_key_deletes_the_misfiled_entry_without_returning_it() {
    let mut map = map_with_ghost();
    assert_eq!(map.remove("other").unwrap(), None);
    assert_eq!(map.len().unwrap(), 1);
}

#[test]
fn an_honest_key_still_reads_and_removes_beside_a_misfiled_map_entry() {
    let mut map = map_with_ghost();
    assert_eq!(map.get("keep").unwrap().map(|value| *value), Some(1));
    assert_eq!(map.remove("keep").unwrap(), Some(1));
    assert!(map.get("keep").unwrap().is_none());
    assert_eq!(map.entries().unwrap().count(), 0);
}

#[test]
fn an_entry_at_that_key_refiles_the_misfiled_entry_keeping_its_value() {
    let mut map = map_with_ghost();
    drop(map.entry("other".to_owned()).unwrap().or_insert(2).unwrap());
    assert_eq!(
        sorted(map.entries().unwrap()),
        [("keep".to_owned(), 1), ("other".to_owned(), 9)]
    );
}

#[test]
fn a_rekey_leaves_a_misfiled_map_entry_behind() {
    let mut map = map_with_ghost();
    map.reassign_deterministic_id("moved");
    assert_eq!(sorted(map.entries().unwrap()), [("keep".to_owned(), 1)]);
    assert_eq!(map.len().unwrap(), 1);
}

/// A peer's map holding a register for "ghost" filed where "other" lives, stamped
/// newer than anything written locally.
fn registers_with_misfiled_ghost() -> Registers {
    env::reset_for_testing();
    // A register's entry bytes as a map writes them, taken from an honest insert.
    let mut scratch = Registers::new_with_field_name("scratch");
    assert!(scratch
        .insert("ghost".to_owned(), LwwRegister::new(9))
        .unwrap()
        .is_none());
    let stored = MainInterface::find_by_id_raw(scratch.entry_id("ghost")).unwrap();
    let (item, _id) = stored.split_at(stored.len() - size_of::<Id>());

    let theirs = Registers::new_with_field_name("theirs");
    let newer = env::time_now() + 1_000_000_000;
    let mut misfiled = row(theirs.id(), theirs.entry_id("other"), &());
    if let Action::Add { data, metadata, .. } = &mut misfiled {
        data.splice(..0, item.iter().copied());
        *metadata = Metadata::new(newer, newer);
    }
    MainInterface::apply_action(misfiled, &ApplyContext::empty()).unwrap();
    assert_eq!(
        theirs.len().unwrap(),
        1,
        "the misfiled entry must have landed"
    );
    theirs
}

#[test]
fn a_merge_does_not_take_a_misfiled_map_entry() {
    let theirs = registers_with_misfiled_ghost();
    let mut ours = Registers::new_with_field_name("ours");

    ours.merge(&theirs).unwrap();
    assert_eq!(ours.entries().unwrap().count(), 0);
}

#[test]
fn a_merge_does_not_overwrite_a_key_ours_holds_with_a_misfiled_entry() {
    let theirs = registers_with_misfiled_ghost();
    let mut ours = Registers::new_with_field_name("ours");
    assert!(ours
        .insert("ghost".to_owned(), LwwRegister::new(1))
        .unwrap()
        .is_none());

    ours.merge(&theirs).unwrap();
    assert_eq!(
        ours.get("ghost").unwrap().map(|value| *value.get()),
        Some(1)
    );
}

#[test]
fn a_sorted_map_entry_filed_under_another_keys_id_is_not_listed() {
    let map = sorted_map_with_ghost();
    assert_eq!(
        map.entries().unwrap().collect::<Vec<_>>(),
        [("keep".to_owned(), 1)]
    );
}

#[test]
fn a_sorted_map_write_at_that_key_replaces_the_misfiled_entry() {
    let mut map = sorted_map_with_ghost();
    assert_eq!(map.insert("other".to_owned(), 2).unwrap(), None);
    assert_eq!(
        map.entries().unwrap().collect::<Vec<_>>(),
        [("keep".to_owned(), 1), ("other".to_owned(), 2)]
    );
}

#[test]
fn a_sorted_map_get_does_not_return_a_misfiled_entry() {
    let map = sorted_map_with_ghost();
    assert!(map.get("other").unwrap().is_none());
}

#[test]
fn a_sorted_map_get_mut_does_not_return_a_misfiled_entry() {
    let mut map = sorted_map_with_ghost();
    assert!(map.get_mut("other").unwrap().is_none());
}

#[test]
fn a_sorted_map_contains_does_not_see_a_misfiled_entry() {
    let map = sorted_map_with_ghost();
    assert!(!map.contains("other").unwrap());
}

#[test]
fn a_sorted_map_remove_deletes_the_misfiled_entry_without_returning_it() {
    let mut map = sorted_map_with_ghost();
    assert_eq!(map.remove("other").unwrap(), None);
    assert_eq!(map.len().unwrap(), 1);
}

#[test]
fn an_honest_key_still_reads_and_removes_beside_a_misfiled_sorted_map_entry() {
    let mut map = sorted_map_with_ghost();
    assert_eq!(map.get("keep").unwrap().map(|value| *value), Some(1));
    assert_eq!(map.remove("keep").unwrap(), Some(1));
    assert_eq!(map.entries().unwrap().count(), 0);
}

#[test]
fn a_sorted_map_entry_at_that_key_refiles_the_misfiled_entry_keeping_its_value() {
    let mut map = sorted_map_with_ghost();
    drop(map.entry("other".to_owned()).unwrap().or_insert(2).unwrap());
    assert_eq!(
        map.entries().unwrap().collect::<Vec<_>>(),
        [("keep".to_owned(), 1), ("other".to_owned(), 9)]
    );
}

#[test]
fn an_ordered_read_leaves_out_a_misfiled_sorted_map_entry_and_sees_its_refiling() {
    let mut map = sorted_map_with_ghost();
    // Builds the ordered index, which must not hold the misfiled entry.
    assert_eq!(map.last().unwrap(), Some(("keep".to_owned(), 1)));

    assert_eq!(map.insert("other".to_owned(), 2).unwrap(), None);
    assert_eq!(map.last().unwrap(), Some(("other".to_owned(), 2)));
}

#[test]
fn a_set_value_filed_under_another_values_id_is_not_listed() {
    let set = set_holding("ghost");
    assert_eq!(listed(&set), ["keep"]);
}

#[test]
fn a_contains_does_not_see_a_misfiled_set_value() {
    let set = set_holding("ghost");
    assert!(!set.contains("other").unwrap());
}

#[test]
fn a_remove_at_that_value_deletes_the_misfiled_value_without_reporting_it() {
    let mut set = set_holding("ghost");
    assert!(!set.remove("other").unwrap());
    assert_eq!(set.len().unwrap(), 1);
}

#[test]
fn an_insert_at_that_value_refiles_the_misfiled_value() {
    let mut set = set_holding("ghost");
    assert!(set.insert("other".to_owned()).unwrap());
    assert_eq!(listed(&set), ["keep", "other"]);
}

#[test]
fn an_honest_value_still_reads_and_removes_beside_a_misfiled_set_value() {
    let mut set = set_holding("ghost");
    assert!(set.contains("keep").unwrap());
    assert!(set.remove("keep").unwrap());
    assert!(listed(&set).is_empty());
}

#[test]
fn a_misfiled_set_value_equal_to_a_real_one_is_listed_once() {
    let set = set_holding("keep");
    assert_eq!(listed(&set), ["keep"]);
}

#[test]
fn a_rekey_leaves_a_misfiled_set_value_behind() {
    let mut set = set_holding("ghost");
    set.reassign_deterministic_id("moved");
    assert_eq!(listed(&set), ["keep"]);
    assert_eq!(set.len().unwrap(), 1);
}

#[test]
fn a_sorted_set_contains_does_not_see_a_misfiled_value() {
    let set = sorted_set_with_ghost();
    assert!(!set.contains("other").unwrap());
}

#[test]
fn a_sorted_set_insert_refiles_a_value_filed_under_its_id() {
    let mut set = sorted_set_with_ghost();
    assert!(set.insert("other".to_owned()).unwrap());
    assert_eq!(set.iter().unwrap().collect::<Vec<_>>(), ["keep", "other"]);
}

#[test]
fn an_honest_value_still_reads_and_removes_beside_a_misfiled_sorted_set_value() {
    let mut set = sorted_set_with_ghost();
    assert!(set.contains("keep").unwrap());
    assert!(set.remove("keep").unwrap());
    assert_eq!(set.iter().unwrap().count(), 0);
}

#[test]
fn a_counter_does_not_sum_a_misfiled_slot() {
    let counter = synced(
        || GCounter::new_with_field_name("hits"),
        |counter| {
            counter.increment_for(&[1; 32]).unwrap();
            vec![row(
                counter.positive.id(),
                counter.positive.entry_id("other"),
                &(100_u64, "ghost".to_owned()),
            )]
        },
    );
    assert_eq!(
        counter.positive.len().unwrap(),
        2,
        "the misfiled slot must have landed"
    );
    assert_eq!(counter.value().unwrap(), 1);
}

/// An ordered index as one built before misfiled entries were left out: a row
/// naming `ghost` at `filed_at`, under a marker that is the bare `full_hash`.
fn index_as_built_before(collection: Id, ghost: &[u8], filed_at: Id) {
    assert!(MainStorage::index_put(collection, ghost, filed_at));
    let (full, _) = Index::<MainStorage>::get_hashes_for(collection)
        .unwrap()
        .unwrap();
    assert!(MainStorage::index_meta_put(collection, &full));
}

#[test]
fn a_sorted_map_index_built_before_is_rebuilt_without_the_misfiled_entry() {
    let map = sorted_map_with_ghost();
    index_as_built_before(map.id(), b"zghost", map.entry_id("other"));
    assert_eq!(map.last().unwrap(), Some(("keep".to_owned(), 1)));
}

#[test]
fn a_sorted_set_index_built_before_is_rebuilt_without_the_misfiled_value() {
    let set = sorted_set_with_ghost();
    index_as_built_before(
        compute_collection_id(None, "sorted_set"),
        b"zghost",
        set.entry_id("other"),
    );
    assert_eq!(set.last().unwrap(), Some("keep".to_owned()));
}
