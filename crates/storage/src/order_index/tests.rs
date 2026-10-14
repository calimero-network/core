use std::collections::BTreeSet;

use serial_test::serial;

use super::{before_change_at, current, read_header, record, SortKey, Tree, INNER_MAX, LEAF_MAX};
use crate::address::Id;
use crate::child_trie::ChildTrie;
use crate::collections::Vector;
use crate::domain::Domain;
use crate::entities::{ChildInfo, Metadata};
use crate::env;
use crate::index::{EntityIndex, Index};
use crate::store::{Key, MainStorage, StorageAdaptor};

/// A deterministic stream of numbers, so a failure replays.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

fn child(created_at: u64, order: u64) -> ChildInfo {
    let metadata = Metadata {
        created_at,
        order,
        ..Metadata::default()
    };
    ChildInfo::new(Id::random(), [7; 32], metadata)
}

/// A stand-in for the trie root after `n` changes: the tree only compares it.
fn root(n: u64) -> [u8; 32] {
    let mut root = [0xAA; 32];
    root[..8].copy_from_slice(&n.to_le_bytes());
    root
}

/// The tree of `parent` as its header stands, current or not.
fn tree(parent: Id) -> Tree {
    let header = read_header::<MainStorage>(parent).expect("a header");
    Tree {
        parent,
        top: header.top,
    }
}

fn assert_answers_as(parent: Id, model: &BTreeSet<SortKey>, at: &str) {
    let tree = tree(parent);
    for (position, key) in model.iter().enumerate() {
        assert_eq!(
            tree.nth::<MainStorage>(position),
            Ok(Some(key.id)),
            "{at}: nth({position})"
        );
        assert_eq!(
            tree.position_of::<MainStorage>(*key),
            Ok(Some(position)),
            "{at}: position_of the key at {position}",
        );
    }
    assert_eq!(
        tree.nth::<MainStorage>(model.len()),
        Ok(None),
        "{at}: past the end"
    );
}

/// Changes the tree as a link (`linked`) or unlink (`unlinked`) under the trie
/// root `changes` would, and moves `changes` past it.
fn change(parent: Id, changes: &mut u64, unlinked: Option<&ChildInfo>, linked: Option<&ChildInfo>) {
    let pending = before_change_at::<MainStorage>(parent, root(*changes)).expect("current");
    *changes += 1;
    pending.finish::<MainStorage>(root(*changes), unlinked, linked);
}

/// The tree answers `nth` and `position_of` as the sorted list of its keys
/// would, through enough links and unlinks to split leaves and inner nodes and
/// to empty them again, with keys that tie on `created_at` and on `order`.
#[test]
#[serial]
fn the_tree_answers_as_a_sorted_list_through_links_and_unlinks() {
    env::reset_for_testing();
    let parent = Id::random();
    let mut rng = Lcg(7);
    let mut model = BTreeSet::new();
    let mut linked: Vec<ChildInfo> = Vec::new();
    let mut changes = 0;

    record::<MainStorage>(parent, &Domain::Open, root(changes), &[]);

    // Past one inner node of full leaves, so the top splits as well.
    while linked.len() < LEAF_MAX * INNER_MAX + LEAF_MAX * 4 {
        // A narrow clock, so keys tie on `created_at` and fall to `order`, and
        // a narrower order, so they tie on that too and fall to the id.
        let next = child(rng.next() % 64, rng.next() % 4);
        change(parent, &mut changes, None, Some(&next));
        let _new = model.insert(SortKey::of(&next));
        linked.push(next);
        if rng.next() % 4 == 0 {
            let gone = linked.swap_remove(rng.below(linked.len()));
            change(parent, &mut changes, Some(&gone), None);
            let _removed = model.remove(&SortKey::of(&gone));
        }
    }
    assert_answers_as(parent, &model, "grown");

    while !linked.is_empty() {
        let gone = linked.swap_remove(rng.below(linked.len()));
        change(parent, &mut changes, Some(&gone), None);
        let _removed = model.remove(&SortKey::of(&gone));
        if linked.len() % 2_000 == 0 {
            assert_answers_as(parent, &model, "shrinking");
        }
    }
    assert_answers_as(parent, &model, "emptied");
}

/// A change the tree cannot carry, because it was not exact (a link of a key
/// it holds, an unlink of one it does not), leaves it stale rather than wrong.
#[test]
#[serial]
fn a_change_the_tree_cannot_carry_leaves_it_stale() {
    env::reset_for_testing();
    let parent = Id::random();
    let held = child(1, 0);
    record::<MainStorage>(parent, &Domain::Open, root(0), &[held.clone()]);

    let mut changes = 0;
    change(parent, &mut changes, None, Some(&held));
    assert!(
        before_change_at::<MainStorage>(parent, root(changes)).is_none(),
        "a key linked twice"
    );

    record::<MainStorage>(parent, &Domain::Open, root(changes), &[]);
    change(parent, &mut changes, Some(&held), None);
    assert!(
        before_change_at::<MainStorage>(parent, root(changes)).is_none(),
        "a key unlinked that was never linked"
    );
}

/// A contract call's view of `vector`: as stored, nothing cached.
fn fresh(vector: &Vector<u64>) -> Vector<u64> {
    borsh::from_slice(&borsh::to_vec(vector).expect("encode")).expect("decode")
}

/// Every position of a fresh `vector` reads as loading all its children would
/// order them, and the tree that answered is current afterwards.
fn assert_positions(vector: &Vector<u64>, at: &str) {
    let parent = vector.collection_id();
    let order: Vec<Id> = ChildTrie::<MainStorage>::new(parent)
        .children()
        .iter()
        .map(ChildInfo::id)
        .collect();
    for (position, id) in order.iter().enumerate() {
        assert_eq!(
            fresh(vector).entry_id_at(position).expect("nth"),
            Some(*id),
            "{at}: nth({position})"
        );
        assert_eq!(
            fresh(vector).position_of_id(*id).expect("position_of"),
            Some(position),
            "{at}: position_of the child at {position}"
        );
    }
    assert_eq!(
        fresh(vector).entry_id_at(order.len()).expect("nth"),
        None,
        "{at}: past the end"
    );
    assert!(
        current::<MainStorage>(parent, &Domain::Open).is_ok(),
        "{at}: the tree is current"
    );
}

/// A vector's positional reads come from the tree once one is built, and the
/// tree follows every link and unlink after that, local or applied, so a read
/// never has to load the children again; a link that bypasses it (snapshot
/// install) makes it stale, and the next read rebuilds it.
#[test]
#[serial]
fn a_vector_reads_positions_from_the_tree_as_its_children_change() {
    env::reset_for_testing();
    let mut vector = Vector::<u64>::new();
    for n in 0..=LEAF_MAX as u64 {
        vector.push(n).expect("push");
    }
    let parent = vector.collection_id();
    assert!(
        current::<MainStorage>(parent, &Domain::Open).is_err(),
        "no tree until a positional read needs one"
    );
    assert_positions(&vector, "first read");

    let current_after = |at: &str| {
        assert!(
            current::<MainStorage>(parent, &Domain::Open).is_ok(),
            "{at}: carried across, not left for a rebuild"
        );
    };

    for n in 0..3 {
        vector.push(1_000 + n).expect("push");
    }
    current_after("local pushes");
    assert_positions(&vector, "local pushes");

    // A peer's entry written before ours, as a delta links it: it sorts first.
    Index::<MainStorage>::add_child_to(parent, child(1, 0)).expect("link");
    current_after("an applied link");
    assert_positions(&vector, "an applied link");

    let middle = fresh(&vector)
        .entry_id_at(LEAF_MAX / 2)
        .expect("nth")
        .expect("in bounds");
    Index::<MainStorage>::remove_child_from(parent, middle, 1).expect("unlink");
    current_after("an unlink");
    assert_positions(&vector, "an unlink");

    // As snapshot install does it: the entity's own index row, then the trie
    // rows, neither through `add_child_to`.
    let behind = child(2, 0);
    let mut index = EntityIndex::minimal_for_test_with_parent(behind.id(), parent, [9; 32]);
    index.metadata = behind.metadata.clone();
    let _written = MainStorage::storage_write(
        Key::Index(behind.id()),
        &borsh::to_vec(&index).expect("encode index"),
    );
    ChildTrie::<MainStorage>::insert_with(
        parent,
        behind,
        |key: Key| MainStorage::storage_read(key),
        |key: Key, value: &[u8]| {
            let _written = MainStorage::storage_write(key, value);
        },
    );
    assert!(
        current::<MainStorage>(parent, &Domain::Open).is_err(),
        "a link the tree did not see"
    );
    assert_positions(&vector, "rebuilt after a link it did not see");
}
