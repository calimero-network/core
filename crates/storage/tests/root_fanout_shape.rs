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

#[test]
fn root_fans_out_flat_when_records_carry_nested_collections() {
    // Mirrors a chat Message: a record whose fields are themselves collections.
    let mut records: Root<UnorderedMap<[u8; 4], u64>> = Root::new(UnorderedMap::new);

    let (base_direct, _, _) = shape();

    // 50 records, each constructing 3 nested collections, as a Message does.
    let mut keep: Vec<(UnorderedSet<[u8; 8]>, Vector<u64>, Vector<u64>)> = Vec::new();
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
