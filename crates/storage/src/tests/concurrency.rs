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

use serial_test::serial;

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

    // These tests drive `Index::add_child_to` directly and assert on the index
    // children list; they don't exercise the delta stream. Opt out of it so a
    // stray `Action::Compare` push can't touch thread-local delta state.
    fn participates_in_sync() -> bool {
        false
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

/// Seed `root` (top of tree) and place `map` under it, single-threaded. The
/// production execute/sync race we model is only on the concurrent leaf inserts.
fn seed_root_and_map(root: Id, map: Id) {
    use crate::action::Action;
    use crate::interface::{ApplyContext, Interface};

    Index::<SharedStore>::add_root(ChildInfo::new(root, [0u8; 32], Metadata::new(1, 1)))
        .expect("seed root index");
    // `root` is already in the index, so the ancestor ChildInfo's hash is never
    // consulted (the ancestor loop skips present entries); `[0; 32]` is fine.
    Interface::<SharedStore>::apply_action(
        Action::Add {
            id: map,
            data: b"map-collection".to_vec(),
            ancestors: vec![ChildInfo::new(root, [0u8; 32], Metadata::new(1, 1))],
            metadata: Metadata::new(2, 2),
        },
        &ApplyContext::empty(),
    )
    .expect("seed map under root");
}

/// Apply a single leaf under `map` via the production `apply_action` path (the
/// same entry point the execute and HashComparison-apply paths both use).
fn apply_leaf_under_map(map: Id, id: Id, data: Vec<u8>, ts: u64) {
    use crate::action::Action;
    use crate::interface::{ApplyContext, Interface};

    Interface::<SharedStore>::apply_action(
        Action::Add {
            id,
            data,
            ancestors: vec![ChildInfo::new(map, [0u8; 32], Metadata::new(2, 2))],
            metadata: Metadata::new(ts, ts),
        },
        &ApplyContext::empty(),
    )
    .expect("apply leaf under map");
}

/// **core#2571 root-cause repro.** Two threads concurrently `add_child_to` the
/// same parent collection (the execute path vs. the HashComparison apply path).
/// Every inserted child has a distinct id, so a correct children index must end
/// up with ALL of them. The read-modify-write race drops some: the parent's
/// `children` list ends shorter than the number of inserts, which in production
/// is the collection entity whose `full_hash` can no longer match the peer.
///
/// `#[serial]`: both tests in this module share the process-global
/// [`shared_store`], so they must not run concurrently with each other (one's
/// `reset_shared_store` would clear the other's data mid-run).
#[test]
#[serial]
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
#[serial]
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

/// **core#2571 — negative result: the composite `apply_action` insert path is
/// NOT the residual divergence source.**
///
/// #2573 made `Index::add_child_to` itself atomic (the two tests above, which
/// call it directly, prove that). The natural next suspect was one level up:
/// production never calls `add_child_to` in isolation — the execute path and
/// the HashComparison apply path both go through `Interface::apply_action`,
/// which performs a *multi-step* index read-modify-write for a single
/// `Action::Add` of a nested entity — `add_child_to(parent, placeholder)` →
/// `save_internal` → `add_child_to(parent, real_hash)` (interface.rs) — each
/// step taking and releasing the #2573 guard *independently*. So the composite
/// is not atomic against a concurrent `apply_action` on the same parent.
///
/// This test drives exactly that interleave: two threads concurrently
/// `apply_action(Action::Add { leaf })` for *distinct* leaves under the same
/// nested map, compared against a serial baseline. It **converges** (asserts
/// equal root hashes) and is robust across many runs — because `add_child_to`
/// re-reads and merges the live `children` list under the guard rather than
/// overwriting it, every interleaving of pure inserts ends with the full child
/// set. The live #2571 split-brain therefore does NOT originate in concurrent
/// nested *inserts* at the index layer; it lies in the value-merge / re-apply
/// path (e.g. a concurrent container `Action::Update` whose `save_internal`
/// short-circuit on equal/stale `updated_at` skips the parent-hash recompute,
/// or a non-idempotent CRDT value merge). Kept as a regression guard so a
/// future change to the composite path can't silently reintroduce child loss.
#[test]
#[serial]
fn concurrent_apply_action_nested_inserts_converge() {
    let root = Id::new([0x11; 32]);
    let map = Id::new([0x22; 32]);

    // Distinct (id, data, timestamp) per leaf: distinct data ⇒ distinct
    // own_hash, so a correct map ends with every leaf contributing to its
    // (and the root's) Merkle hash.
    const LEAVES: u16 = 200;
    let leaves: Vec<(Id, Vec<u8>, u64)> = (0..LEAVES)
        .map(|i| {
            let mut data = vec![0u8; 8];
            data[0] = (i >> 8) as u8;
            data[1] = (i & 0xff) as u8;
            (child_id(0xDD, i), data, 100 + i as u64)
        })
        .collect();

    // Serial baseline: one writer applies every leaf.
    reset_shared_store();
    seed_root_and_map(root, map);
    for (id, data, ts) in &leaves {
        apply_leaf_under_map(map, *id, data.clone(), *ts);
    }
    let (serial_root, _) = Index::<SharedStore>::get_hashes_for(root)
        .expect("root hashes")
        .expect("root present");
    assert_eq!(
        Index::<SharedStore>::get_children_of(map).unwrap().len(),
        leaves.len(),
        "serial baseline must keep every leaf under the map",
    );

    // Concurrent: two threads split the same leaves and apply them through
    // `apply_action` simultaneously — the execute-vs-HashComparison interleave.
    reset_shared_store();
    seed_root_and_map(root, map);
    let (first, second): (Vec<_>, Vec<_>) = leaves
        .iter()
        .cloned()
        .enumerate()
        .partition(|(i, _)| i % 2 == 0);
    let first: Vec<_> = first.into_iter().map(|(_, l)| l).collect::<Vec<_>>();
    let second: Vec<_> = second.into_iter().map(|(_, l)| l).collect::<Vec<_>>();
    let t_execute = std::thread::spawn(move || {
        for (id, data, ts) in &first {
            apply_leaf_under_map(map, *id, data.clone(), *ts);
        }
    });
    let t_sync = std::thread::spawn(move || {
        for (id, data, ts) in &second {
            apply_leaf_under_map(map, *id, data.clone(), *ts);
        }
    });
    t_execute.join().unwrap();
    t_sync.join().unwrap();

    let (concurrent_root, _) = Index::<SharedStore>::get_hashes_for(root)
        .expect("root hashes")
        .expect("root present");
    let concurrent_children = Index::<SharedStore>::get_children_of(map).unwrap().len();

    assert_eq!(
        concurrent_children,
        leaves.len(),
        "concurrent apply_action dropped a leaf from the map's children index \
         ({} of {}) — the collection's full_hash can no longer match a peer's",
        concurrent_children,
        leaves.len(),
    );
    assert_eq!(
        hex::encode(serial_root),
        hex::encode(concurrent_root),
        "core#2571 residual: applying the identical nested leaves concurrently via \
         apply_action produced a different ROOT full_hash than applying them serially \
         — the same-content / different-root split-brain HashComparison cannot heal",
    );
}

/// Seed a single LWW leaf entity `leaf` under `parent` (already a root in the
/// index) with an initial value, single-threaded.
fn seed_lww_leaf(parent: Id, leaf: Id, value: &[u8]) {
    use crate::action::Action;
    use crate::collections::crdt_meta::CrdtType;
    use crate::interface::{ApplyContext, Interface};

    let mut md = Metadata::new(1, 1);
    md.crdt_type = Some(CrdtType::LwwRegister {
        inner_type: "String".to_string(),
    });
    Interface::<SharedStore>::apply_action(
        Action::Add {
            id: leaf,
            data: value.to_vec(),
            ancestors: vec![ChildInfo::new(parent, [0u8; 32], Metadata::new(1, 1))],
            metadata: md,
        },
        &ApplyContext::empty(),
    )
    .expect("seed lww leaf");
}

/// Apply an `Action::Update` to `leaf` carrying `value` at HLC `ts`, via the
/// production `apply_action` → `save_internal` → CRDT-merge path.
fn update_lww_leaf(leaf: Id, value: Vec<u8>, ts: u64) {
    use crate::action::Action;
    use crate::collections::crdt_meta::CrdtType;
    use crate::interface::{ApplyContext, Interface};

    let mut md = Metadata::new(ts, ts);
    md.crdt_type = Some(CrdtType::LwwRegister {
        inner_type: "String".to_string(),
    });
    Interface::<SharedStore>::apply_action(
        Action::Update {
            id: leaf,
            data: value,
            ancestors: vec![],
            metadata: md,
        },
        &ApplyContext::empty(),
    )
    .expect("update lww leaf");
}

/// **core#2571 live-bug repro — the value/own_hash split.**
///
/// `save_internal` (and the CRDT-merge helper it calls) persist an entity in two
/// unsynchronised steps: `storage_write(Key::Entry(id), merged)` for the VALUE,
/// then `Index::update_hash_for(id, own_hash, ..)` for the recorded `own_hash`,
/// where `own_hash = Sha256(merged)` is computed *before* the value write. #2573
/// added the mutation guard, but it wraps only `update_hash_for` — the value
/// write sits outside it (the code even notes "the storage layer doesn't
/// serialize concurrent writes anyway"). So when the execute path and the
/// HashComparison-apply path merge the SAME entity concurrently, the last VALUE
/// write and the last `own_hash` write can come from DIFFERENT writers, leaving
/// the stored bytes and their recorded Merkle `own_hash` inconsistent.
///
/// A peer that recomputes the leaf hash from the bytes gets `Sha256(value)`,
/// while this node's index advertises a different `own_hash` — so the parent
/// collection's `full_hash` can never match and HashComparison re-merges the
/// container forever (the stable-but-different root-hash split-brain).
///
/// The invariant asserted is storage-layer, CRDT-agnostic: a present entity's
/// stored bytes must hash to its recorded `own_hash`. Two threads concurrently
/// `Action::Update` the same LWW leaf with churning values; the round is
/// repeated so the non-sticky race is reliably observed.
#[test]
#[serial]
fn concurrent_merge_splits_value_from_own_hash() {
    use sha2::{Digest, Sha256};

    let root = Id::new([0x31; 32]);
    let leaf = Id::new([0x32; 32]);

    const ROUNDS: u16 = 40;
    const PER_THREAD: u16 = 60;
    // `round`/`i` are stamped into the per-write payload as a single `u8`
    // below; keep the counts inside one byte so two distinct rounds/iters can
    // never collide on the same payload pattern. The loops are exclusive, so
    // the largest stamped value is `count - 1`; `<= 256` (not `< 256`) is the
    // correct bound — at `count == 256` the max index is 255, which still fits
    // a `u8`. This byte budget guards ONLY payload uniqueness for the
    // value-winner assertion; it has no bearing on the HLC-ordering guarantee
    // below, which holds for any `PER_THREAD`.
    const _: () = assert!(ROUNDS <= 256, "round as u8 would truncate");
    const _: () = assert!(PER_THREAD <= 256, "i as u8 would truncate");

    for round in 0..ROUNDS {
        reset_shared_store();
        Index::<SharedStore>::add_root(ChildInfo::new(root, [0u8; 32], Metadata::new(1, 1)))
            .expect("seed root");
        seed_lww_leaf(root, leaf, b"genesis");

        // Each writer churns a distinct, monotonically-newer value so every
        // Update is accepted (incoming-newer) and actually writes — maximising
        // the interleave between the value write and the own_hash update. The
        // two threads use interleaved-but-disjoint HLC ranges (execute = even,
        // sync = odd) so writes are *strictly* newer rather than equal-HLC: the
        // merge takes the `incoming_timestamp > existing` branch (the one the
        // bug targets), not the equal-HLC content-hash tiebreak. The global
        // maximum HLC is therefore `t_sync`'s last write, so the converged
        // value is deterministic — asserted after the loop joins.
        let t_execute = std::thread::spawn(move || {
            for i in 0..PER_THREAD {
                let ts = 100 + i as u64 * 2;
                update_lww_leaf(leaf, vec![0xAA, round as u8, i as u8], ts);
            }
        });
        let t_sync = std::thread::spawn(move || {
            for i in 0..PER_THREAD {
                let ts = 101 + i as u64 * 2;
                update_lww_leaf(leaf, vec![0xBB, round as u8, i as u8], ts);
            }
        });
        t_execute.join().unwrap();
        t_sync.join().unwrap();

        let stored = SharedStore::storage_read(Key::Entry(leaf)).expect("leaf value present");
        let stored_hash: [u8; 32] = Sha256::digest(&stored).into();
        let (_full, own_hash) = Index::<SharedStore>::get_hashes_for(leaf)
            .expect("leaf hashes")
            .expect("leaf index present");

        assert_eq!(
            hex::encode(stored_hash),
            hex::encode(own_hash),
            "core#2571 (round {round}): the stored entity bytes hash to {} but the \
             Merkle index records own_hash {} — concurrent merges split the value \
             write from the own_hash update, so peers can never match this leaf's \
             contribution to the collection full_hash",
            hex::encode(stored_hash),
            hex::encode(own_hash),
        );

        // The converged value must be the global LWW winner, not merely
        // self-consistent: `t_sync`'s highest HLC (101 + 2*(PER_THREAD-1))
        // strictly exceeds every `t_execute` write (100 + 2*(PER_THREAD-1)).
        // `lww_pick` resolves by HLC magnitude, not arrival order — a lower-ts
        // write loses whether it arrives as `incoming` or sits as `existing` —
        // so the leaf settles on that single highest-ts payload no matter how
        // the two threads interleave. This is a second, independent invariant
        // from the hash check above: it catches a "hash is self-consistent but
        // the bytes are a stale writer's" regression the hash check can't see.
        let expected = vec![0xBB, round as u8, (PER_THREAD - 1) as u8];
        assert_eq!(
            stored, expected,
            "core#2571 (round {round}): converged on the wrong writer's value — \
             expected the global LWW winner (t_sync's last write) but stored {:?}",
            stored,
        );
    }
}
