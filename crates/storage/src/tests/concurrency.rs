//! Concurrency reproduction harness for core#2571.
//!
//! The opaque-leaf sync-regression smoke test (Round 2) intermittently
//! split-brains: two nodes settle on stable-but-different Merkle root hashes and
//! HashComparison re-merges the same collection entity forever. Every
//! single-threaded reproduction converges, which points at a **concurrency
//! race**: in production a local write (the execute path) and a HashComparison
//! apply (the dedicated `SyncSessionActor`, #2316) run on different tasks and
//! both mutate the same collection's Merkle `children` index.
//!
//! `Index::add_child_to` is a read-modify-write on the parent's index entry
//! (`get_index` → mutate the `children` Vec → `save_index`). With two
//! concurrent writers and no per-parent lock, that is a lost-update window: one
//! writer's freshly-added child is overwritten by the other's stale snapshot.
//! A collection that loses a child from its `children` list (while the child's
//! `Key::Entry` still exists) produces a `full_hash` that no peer can match —
//! the irreconcilable divergence HashComparison loops on.
//!
//! These tests drive that race against a **shared, cross-thread** storage
//! backend (the production `MockedStorage` is `thread_local!`, so it can't model
//! two actors hitting one RocksDB).

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::address::Id;
use crate::entities::{ChildInfo, Metadata};
use crate::index::Index;
use crate::store::{Key, StorageAdaptor};

/// Process-global key/value store shared across threads, standing in for the
/// single RocksDB column two actors concurrently read/write in production. Each
/// `storage_*` call locks independently — exactly the granularity that leaves a
/// read-modify-write across two calls unprotected.
fn shared_store() -> &'static Mutex<BTreeMap<Key, Vec<u8>>> {
    static STORE: OnceLock<Mutex<BTreeMap<Key, Vec<u8>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn reset_shared_store() {
    shared_store().lock().unwrap().clear();
}

/// A `StorageAdaptor` backed by the cross-thread [`shared_store`].
struct SharedStore;

impl StorageAdaptor for SharedStore {
    fn storage_read(key: Key) -> Option<Vec<u8>> {
        shared_store().lock().unwrap().get(&key).cloned()
    }

    fn storage_remove(key: Key) -> bool {
        shared_store().lock().unwrap().remove(&key).is_some()
    }

    fn storage_write(key: Key, value: &[u8]) -> bool {
        shared_store()
            .lock()
            .unwrap()
            .insert(key, value.to_vec())
            .is_some()
    }
}

/// Deterministic distinct child id: group byte + 2-byte counter.
fn child_id(group: u8, i: u16) -> Id {
    let mut bytes = [0u8; 32];
    bytes[0] = group;
    bytes[1] = (i >> 8) as u8;
    bytes[2] = (i & 0xff) as u8;
    Id::new(bytes)
}

/// **core#2571 root-cause repro.** Two threads concurrently `add_child_to` the
/// same parent collection (the execute path vs. the HashComparison apply path).
/// Every inserted child has a distinct id, so a correct children index must end
/// up with ALL of them. The read-modify-write race drops some: the parent's
/// `children` list ends shorter than the number of inserts, which in production
/// is the collection entity whose `full_hash` can no longer match the peer.
#[test]
fn concurrent_add_child_to_loses_children() {
    reset_shared_store();

    let parent = Id::new([1u8; 32]);
    Index::<SharedStore>::add_root(ChildInfo::new(parent, [0u8; 32], Metadata::new(1, 1)))
        .expect("seed parent index");

    const PER_THREAD: u16 = 300;

    let t_execute = std::thread::spawn(move || {
        for i in 0..PER_THREAD {
            let id = child_id(0xAA, i);
            Index::<SharedStore>::add_child_to(
                parent,
                ChildInfo::new(
                    id,
                    [i as u8; 32],
                    Metadata::new(100 + i as u64, 100 + i as u64),
                ),
            )
            .expect("execute add_child_to");
        }
    });
    let t_sync = std::thread::spawn(move || {
        for i in 0..PER_THREAD {
            let id = child_id(0xBB, i);
            Index::<SharedStore>::add_child_to(
                parent,
                ChildInfo::new(
                    id,
                    [i as u8; 32],
                    Metadata::new(200 + i as u64, 200 + i as u64),
                ),
            )
            .expect("sync add_child_to");
        }
    });
    t_execute.join().unwrap();
    t_sync.join().unwrap();

    let children = Index::<SharedStore>::get_children_of(parent).expect("read children");
    let expected = (PER_THREAD as usize) * 2;

    assert_eq!(
        children.len(),
        expected,
        "lost-update race: parent's children index holds {} of {} concurrently-added \
         children — a collection that loses children from its Merkle index produces a \
         full_hash no peer can match (core#2571 HashComparison sticky-loop)",
        children.len(),
        expected,
    );
}

/// The direct symptom of the race: a parent that received its children
/// concurrently ends with a DIFFERENT `full_hash` than one that received the
/// exact same children serially. In production the "serial" node is the writer
/// (local execute, single-threaded) and the "concurrent" node is the one whose
/// `SyncSessionActor` apply raced its own execute path — so their collection
/// hashes never match and HashComparison can't heal it.
#[test]
fn concurrent_children_index_diverges_from_serial_full_hash() {
    // The exact same set of children, with identical ids/metadata.
    let children: Vec<ChildInfo> = (0..200u16)
        .map(|i| {
            ChildInfo::new(
                child_id(0xCC, i),
                [i as u8; 32],
                Metadata::new(100 + i as u64, 100 + i as u64),
            )
        })
        .collect();

    let parent = Id::new([2u8; 32]);

    // Baseline: add all children serially (the single-threaded writer).
    reset_shared_store();
    Index::<SharedStore>::add_root(ChildInfo::new(parent, [9u8; 32], Metadata::new(1, 1)))
        .expect("seed parent");
    for c in &children {
        Index::<SharedStore>::add_child_to(parent, c.clone()).expect("serial add");
    }
    let (serial_full, _) = Index::<SharedStore>::get_hashes_for(parent)
        .expect("hashes")
        .expect("present");
    let serial_len = Index::<SharedStore>::get_children_of(parent).unwrap().len();
    assert_eq!(
        serial_len,
        children.len(),
        "serial baseline must keep all children"
    );

    // Concurrent: two threads split the same children set.
    reset_shared_store();
    Index::<SharedStore>::add_root(ChildInfo::new(parent, [9u8; 32], Metadata::new(1, 1)))
        .expect("seed parent");
    let (first, second): (Vec<_>, Vec<_>) = children
        .iter()
        .cloned()
        .enumerate()
        .partition(|(i, _)| i % 2 == 0);
    let first: Vec<ChildInfo> = first.into_iter().map(|(_, c)| c).collect();
    let second: Vec<ChildInfo> = second.into_iter().map(|(_, c)| c).collect();
    let h1 = std::thread::spawn(move || {
        for c in &first {
            Index::<SharedStore>::add_child_to(parent, c.clone()).expect("t1 add");
        }
    });
    let h2 = std::thread::spawn(move || {
        for c in &second {
            Index::<SharedStore>::add_child_to(parent, c.clone()).expect("t2 add");
        }
    });
    h1.join().unwrap();
    h2.join().unwrap();
    let (concurrent_full, _) = Index::<SharedStore>::get_hashes_for(parent)
        .expect("hashes")
        .expect("present");

    assert_eq!(
        hex::encode(serial_full),
        hex::encode(concurrent_full),
        "core#2571: a collection whose children were added concurrently computed a \
         different full_hash than one that received the identical children serially — \
         this is the divergent collection hash HashComparison re-merges forever",
    );
}
