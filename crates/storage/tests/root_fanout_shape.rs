//! What shape does the entity index actually take when records carry nested
//! collections?
//!
//! `Collection::new` registers every new collection as a child of ROOT
//! (`collections.rs:340`), and re-keying re-adds it (`collections.rs:542`).
//! This measures the consequence rather than arguing about it.

use calimero_storage::address::Id;
use calimero_storage::collections::{Root, UnorderedMap, UnorderedSet, Vector};
use calimero_storage::index::Index;
use calimero_storage::store::MainStorage;

/// Count ROOT's direct children and the depth of the tree beneath it.
fn shape() -> (usize, usize, usize) {
    let root = Id::root();
    let children = <Index<MainStorage>>::get_children_of(root).expect("children");
    let direct = children.len();

    // Walk down to find the deepest path and the total node count.
    let mut depth = 0;
    let mut total = 0;
    let mut frontier: Vec<Id> = children.iter().map(|c| c.id()).collect();
    while !frontier.is_empty() {
        depth += 1;
        total += frontier.len();
        let mut next = Vec::new();
        for id in frontier {
            if let Ok(kids) = <Index<MainStorage>>::get_children_of(id) {
                next.extend(kids.iter().map(|c| c.id()));
            }
        }
        frontier = next;
        if depth > 64 {
            break;
        }
    }
    (direct, depth, total)
}

/// The nested collections each record carries, held for the duration of the
/// profile so their rows stay in the store.
type NestedTriple = (UnorderedSet<[u8; 8]>, Vector<u64>, Vector<u64>);

#[test]
fn root_fans_out_flat_when_records_carry_nested_collections() {
    // Mirrors a chat Message: a record whose fields are themselves collections.
    let mut records: Root<UnorderedMap<[u8; 4], u64>> = Root::new(UnorderedMap::new);

    let (base_direct, _, _) = shape();

    // 50 records, each constructing 3 nested collections, as a Message does.
    let mut keep: Vec<NestedTriple> = Vec::new();
    for i in 0_u32..50 {
        let mut set = UnorderedSet::<[u8; 8]>::new();
        let _ = set.insert(u64::from(i).to_be_bytes());
        let mut a = Vector::<u64>::new();
        let _ = a.push(u64::from(i));
        let mut b = Vector::<u64>::new();
        let _ = b.push(u64::from(i));
        keep.push((set, a, b));
        let _ = records.insert(i.to_be_bytes(), u64::from(i));
    }

    let (direct, depth, total) = shape();
    let added = direct - base_direct;

    println!("ROOT direct children: {base_direct} -> {direct} (+{added})");
    println!("tree depth below ROOT: {depth}");
    println!("total nodes below ROOT: {total}");
    println!("records inserted: 50, nested collections constructed: 150");

    // The question this test exists to answer: do those 150 nested collections
    // hang off the records they belong to, or off ROOT?
    assert!(
        added >= 150,
        "expected ~150 nested collections to land directly on ROOT, saw +{added}"
    );
}

/// The parent's index row must not grow with its child count.
///
/// It used to hold the whole child list inline: 16,927 bytes at 200 children,
/// ~84 bytes per child, rewritten and re-hashed on every link. Children now live
/// in the parent's `ChildTrie`, so this row is a fixed handful of fields and a
/// link touches a bounded number of trie rows instead (see
/// `child_trie::cost`).
#[test]
fn the_parent_index_row_stays_small_however_many_children() {
    use calimero_storage::store::{Key, MainStorage, StorageAdaptor};

    let mut keep: Vec<UnorderedSet<[u8; 8]>> = Vec::new();
    let mut sizes = Vec::new();
    for i in 0_u64..200 {
        let mut set = UnorderedSet::<[u8; 8]>::new();
        let _ = set.insert(i.to_be_bytes());
        keep.push(set);
        if i == 9 || i == 49 || i == 199 {
            let raw = MainStorage::storage_read(Key::Index(Id::root())).expect("root index row");
            let n = <Index<MainStorage>>::get_children_of(Id::root())
                .expect("children")
                .len();
            println!("children={n:>4}  parent index row={} bytes", raw.len());
            sizes.push(raw.len());
        }
    }

    assert_eq!(
        sizes.first(),
        sizes.last(),
        "the parent's index row must not grow with its child count: {sizes:?}"
    );
}
