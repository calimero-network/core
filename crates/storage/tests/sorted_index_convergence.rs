//! Cross-node convergence regression for `SortedSet` / `SortedMap` ordered reads
//! (core#2559 follow-up; surfaced via calimero-network/calimero-sdk-js#87).
//!
//! # The bug
//!
//! `SortedSet`/`SortedMap` keep a **node-local** ordered index (RocksDB
//! `Column::SortedIndex`) that sync never touches, guarded by a validity marker
//! holding the collection's `full_hash` when the index was last (re)built. An
//! ordered read (`iter`/`keys`/`range`/`first`/`last`) rebuilds the index only
//! when that marker != the collection's current `full_hash`.
//!
//! The marker used to be written through the **synced** state path
//! (`storage_write` → `Column::State`). So a node that built its index and
//! stamped `marker = H({a,b})` could make that marker observable to a peer whose
//! own node-local index was still stale — the peer saw `marker == full_hash`,
//! skipped the rebuild, and served a **stale subset** of converged data. `len()`
//! / `contains()` read children directly and converged; only the index-backed
//! ordered readers diverged. The fix relocates the marker to a dedicated
//! node-local keyspace (`Column::SortedIndexMeta`) that mirrors the index it
//! guards, so a peer never inherits another node's marker.
//!
//! # Why this lives in `calimero-storage`, not the node-sync harness
//!
//! A faithful reproduction needs two properties at once: (1) two nodes that have
//! **converged state**, and (2) **per-node** ordered indexes (one node built its
//! index, the other has not). The in-process node-sync simulator
//! (`crates/node/tests/sync_sim`) deliberately skips WASM execution and drives
//! storage via `Interface::apply_action` — it never exercises `SortedSet::insert`
//! or the ordered-index/marker path at all — and its native ordered-index mock
//! is a single process-wide thread-local shared across simulated nodes, so it
//! cannot represent a per-node stale index. This test therefore models the two
//! nodes at the storage layer, which is where the marker/index code actually
//! lives: **shared (converged) synced state** via a common `RuntimeEnv` store,
//! plus a **node-local** ordered index that we reset between the two nodes to
//! model a peer that received the data via sync but has not yet built its index.
//! Whether resetting the node-local index also drops the marker is exactly what
//! the fix changes — before it, the marker survived in synced state and lied.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use calimero_storage::collections::{Root, SortedMap, SortedSet};
use calimero_storage::delta::StorageDelta;
use calimero_storage::env::{
    clear_sorted_index_for_testing, drop_sorted_index_entry_for_testing, reset_environment,
    take_last_artifact, with_runtime_env, RuntimeEnv,
};
use calimero_storage::interface::{ApplyContext, Interface};
use calimero_storage::store::{Key, MainStorage};

/// A shared, synced state store — models state that both nodes have converged on.
type SharedState = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;

const CONTEXT_ID: [u8; 32] = [7u8; 32];

/// Build a `RuntimeEnv` routing all `MainStorage` state I/O into `state` under
/// `executor`. The node-local ordered index is NOT part of `RuntimeEnv` — it
/// stays in the process's native index mock, which is what makes "reset the
/// index between nodes, keep the state" a faithful per-node model.
/// `executor` is the DEVICE. Its account is derived from it here so the two
/// nodes in this test are two devices of two different accounts — the ordered
/// index is per-node state and nothing in this scenario is writer-set guarded,
/// so the pairing only has to be consistent, not related to any real binding.
fn env_for(state: &SharedState, executor: [u8; 32]) -> RuntimeEnv {
    let r = Rc::clone(state);
    let reader = Rc::new(move |key: &Key| r.borrow().get(&key.to_bytes()).cloned());
    let w = Rc::clone(state);
    let writer = Rc::new(move |key: Key, value: &[u8]| {
        w.borrow_mut()
            .insert(key.to_bytes(), value.to_vec())
            .is_some()
    });
    let rm = Rc::clone(state);
    let remover = Rc::new(move |key: &Key| rm.borrow_mut().remove(&key.to_bytes()).is_some());
    let account = {
        let mut a = executor;
        a[0] ^= 0xA0; // a different value than the device id, so the two cannot be confused
        a
    };
    RuntimeEnv::new(reader, writer, remover, CONTEXT_ID, executor, account)
}

/// SortedSet: node A builds the set + its index; node B shares node A's converged
/// state but has a fresh (unbuilt) node-local index. Node B's ordered `iter()`
/// MUST return the full set — it must notice its own index is stale and rebuild,
/// not trust a marker node A stamped.
#[test]
fn sorted_set_ordered_read_converges_on_second_node() {
    reset_environment();
    let state: SharedState = Rc::new(RefCell::new(HashMap::new()));

    // Node A: build the set and warm its node-local index + marker.
    with_runtime_env(env_for(&state, [1u8; 32]), || {
        let mut set = Root::new(SortedSet::<String, MainStorage>::new);
        assert!(set.insert("a".to_owned()).unwrap());
        assert!(set.insert("b".to_owned()).unwrap());
        // Node A itself reads correctly.
        assert_eq!(
            set.iter().unwrap().collect::<Vec<_>>(),
            vec!["a".to_owned(), "b".to_owned()]
        );
        set.commit();
    });

    // Model node B: same converged synced state, but a fresh node-local index
    // (sync delivers state, never the index). Before the fix the marker lived in
    // the synced state and survived this reset, so node B skipped its rebuild and
    // served a stale subset; after the fix the marker is node-local and is reset
    // with the index, so node B rebuilds.
    clear_sorted_index_for_testing();

    with_runtime_env(env_for(&state, [2u8; 32]), || {
        let set = Root::<SortedSet<String, MainStorage>>::fetch().expect("node B sees state");
        let got: Vec<String> = set.iter().unwrap().collect();
        assert_eq!(
            got,
            vec!["a".to_owned(), "b".to_owned()],
            "node B's ordered iter() diverged from converged state — the sorted \
             index marker leaked across nodes (regression: marker must be node-local)"
        );
        // first/last are index-backed too — they must also see the full set.
        assert_eq!(set.first().unwrap(), Some("a".to_owned()));
        assert_eq!(set.last().unwrap(), Some("b".to_owned()));
    });
}

/// SortedMap: same shape — node B (converged state, fresh index) must see every
/// key in order via `keys()` / `entries()`.
#[test]
fn sorted_map_ordered_read_converges_on_second_node() {
    reset_environment();
    let state: SharedState = Rc::new(RefCell::new(HashMap::new()));

    with_runtime_env(env_for(&state, [1u8; 32]), || {
        let mut map = Root::new(SortedMap::<String, String, MainStorage>::new);
        assert!(map
            .insert("a".to_owned(), "A".to_owned())
            .unwrap()
            .is_none());
        assert!(map
            .insert("b".to_owned(), "B".to_owned())
            .unwrap()
            .is_none());
        assert_eq!(
            map.keys().unwrap().collect::<Vec<_>>(),
            vec!["a".to_owned(), "b".to_owned()]
        );
        map.commit();
    });

    clear_sorted_index_for_testing();

    with_runtime_env(env_for(&state, [2u8; 32]), || {
        let map =
            Root::<SortedMap<String, String, MainStorage>>::fetch().expect("node B sees state");
        let keys: Vec<String> = map.keys().unwrap().collect();
        assert_eq!(
            keys,
            vec!["a".to_owned(), "b".to_owned()],
            "node B's ordered keys() diverged from converged state — sorted index \
             marker leaked across nodes (regression: marker must be node-local)"
        );
        let entries: Vec<(String, String)> = map.entries().unwrap().collect();
        assert_eq!(
            entries,
            vec![
                ("a".to_owned(), "A".to_owned()),
                ("b".to_owned(), "B".to_owned())
            ]
        );
    });
}

// === Invalidate-on-sync: the apply path must clear the ordered-index marker ===
//
// #3323 made the validity marker node-local, so a peer no longer inherits
// another node's marker. But that only fixes the case where the receiving node's
// index is *fresh* (marker absent → rebuild fires). The real 2-node merobox run
// (sdk-js#87, AFTER #3323 merged) showed a subtler state on the receiving node:
// its index had been built for a *subset* of the element set (`{b}`) while its
// marker had been stamped to the *converged* `full_hash` (`H({a,b})`). So
// `index_marker_current()` returned `true` even though the index content (`{b}`)
// disagreed with the live, fully-synced element set (`{a,b}`):
//   - `contains('a')` = true   (direct child lookup)
//   - `len()`         = 2      (children linked & enumerable)
//   - `iter()`        = ['b']  (stale ordered index served forever)
//
// The `full_hash` marker can therefore run ahead of the enumerable child list,
// and a read-side check cannot both stay O(1) and catch this (`len()` is itself
// O(n)). The fix keeps ordered reads O(1) (marker-only) and moves correctness to
// the *write* side: `Interface::apply_action` / `apply_delete_ref_action` clear
// the parent collection's node-local marker whenever a synced delta links or
// unlinks a child (the non-`insert` path), so the next ordered read rebuilds the
// index once from the converged child set.
//
// These tests reproduce the exact false-positive state
// (`drop_sorted_index_entry_for_testing` leaves index = {b}, children = {a,b},
// marker == current full_hash), assert the O(1) read serves the stale subset
// while the marker matches (the CONTROL — this is what the read alone can never
// fix), then drive the REAL apply choke point by replaying the collection's own
// delta through `Interface::apply_action` (exactly what `Root::sync` does per
// action on the receive path). The child-link apply clears the marker, so the
// next ordered read heals to the full converged set. Remove the
// `index_meta_clear` call from `apply_action` and these tests fail: the read
// stays stale because the marker still matches.
//
// The replay is idempotent (same node, its own already-applied delta), so it
// does NOT change `full_hash` — which is the point: if the apply changed the
// child set the marker would mismatch and rebuild "for free", masking whether
// the invalidation fired. Holding `full_hash` fixed isolates the marker clear as
// the sole thing that heals the read. Root actions are skipped on replay so the
// test exercises the child-link path without the root-state merge (which would
// require a registered `Mergeable`).

/// Replay the non-root (child-link) actions of a captured `StorageDelta` through
/// the real `Interface::apply_action` receive path — the choke point that must
/// clear the parent collection's ordered-index marker.
fn replay_child_links<S: calimero_storage::store::StorageAdaptor>(delta: &[u8]) {
    let actions = match borsh::from_slice::<StorageDelta>(delta).expect("decode delta") {
        StorageDelta::Actions(actions) => actions,
        StorageDelta::CausalActions { actions, .. } => actions,
    };
    for action in actions {
        if action.id().is_root() {
            continue;
        }
        Interface::<S>::apply_action(action, &ApplyContext::empty()).expect("apply_action");
    }
}

/// SortedSet: a synced child-link apply must invalidate the ordered-index marker
/// so a converged-but-stale index rebuilds on the next ordered read — even when
/// the marker still equals the collection's current `full_hash`.
#[test]
fn sorted_set_apply_invalidates_stale_index_marker() {
    reset_environment();
    let state: SharedState = Rc::new(RefCell::new(HashMap::new()));

    // Build the full set + warm its index/marker, and capture the delta the
    // collection emitted (the Add actions a peer would receive & apply).
    let delta = with_runtime_env(env_for(&state, [1u8; 32]), || {
        let mut set = Root::new(SortedSet::<String, MainStorage>::new);
        assert!(set.insert("a".to_owned()).unwrap());
        assert!(set.insert("b".to_owned()).unwrap());
        assert_eq!(
            set.iter().unwrap().collect::<Vec<_>>(),
            vec!["a".to_owned(), "b".to_owned()]
        );
        set.commit();
        take_last_artifact().expect("commit emitted a delta")
    });

    // Manufacture the false positive: index loses 'a' (as if a rebuild ran
    // against a momentarily stale child list) but the marker stays == the
    // converged full_hash and the synced children stay {a,b}.
    drop_sorted_index_entry_for_testing(b"a");

    // CONTROL: the O(1) marker-only read cannot detect this — it serves the
    // stale subset {b} because the marker matches. This is the observed bug.
    with_runtime_env(env_for(&state, [1u8; 32]), || {
        let set = Root::<SortedSet<String, MainStorage>>::fetch().expect("state present");
        assert!(set.contains("a").unwrap(), "child 'a' is present");
        assert_eq!(set.len().unwrap(), 2, "both children enumerable");
        assert_eq!(
            set.iter().unwrap().collect::<Vec<String>>(),
            vec!["b".to_owned()],
            "control: with a matching marker the O(1) read serves the stale subset"
        );
    });

    // Drive the REAL receive/apply path: replay the collection's own child links.
    // Idempotent (same node), so full_hash is unchanged — only the marker clear
    // can heal the read.
    with_runtime_env(env_for(&state, [1u8; 32]), || {
        replay_child_links::<MainStorage>(&delta);
    });

    // After the apply invalidated the marker, the next ordered read rebuilds and
    // serves the full converged set. Fails if `apply_action`'s `index_meta_clear`
    // is removed (marker still matches → stale {b}).
    with_runtime_env(env_for(&state, [1u8; 32]), || {
        let set = Root::<SortedSet<String, MainStorage>>::fetch().expect("state present");
        let got: Vec<String> = set.iter().unwrap().collect();
        assert_eq!(
            got,
            vec!["a".to_owned(), "b".to_owned()],
            "ordered iter() still served a stale subset — the sync/apply path did \
             not invalidate the ordered-index marker (sdk-js#87)"
        );
        // first/last are index-backed too and must agree after the self-heal.
        assert_eq!(set.first().unwrap(), Some("a".to_owned()));
        assert_eq!(set.last().unwrap(), Some("b".to_owned()));
    });
}

/// SortedMap: same shape. `range`/`first`/`last` are the index-backed readers,
/// so they are what a stale index corrupts and what must heal once the apply
/// path invalidates the marker.
#[test]
fn sorted_map_apply_invalidates_stale_index_marker() {
    reset_environment();
    let state: SharedState = Rc::new(RefCell::new(HashMap::new()));

    let delta = with_runtime_env(env_for(&state, [1u8; 32]), || {
        let mut map = Root::new(SortedMap::<String, String, MainStorage>::new);
        assert!(map
            .insert("a".to_owned(), "A".to_owned())
            .unwrap()
            .is_none());
        assert!(map
            .insert("b".to_owned(), "B".to_owned())
            .unwrap()
            .is_none());
        // Warm the index via an index-backed reader.
        assert_eq!(
            map.range(..).unwrap().map(|(k, _)| k).collect::<Vec<_>>(),
            vec!["a".to_owned(), "b".to_owned()]
        );
        map.commit();
        take_last_artifact().expect("commit emitted a delta")
    });

    drop_sorted_index_entry_for_testing(b"a");

    // CONTROL: stale subset served because the marker matches.
    with_runtime_env(env_for(&state, [1u8; 32]), || {
        let map = Root::<SortedMap<String, String, MainStorage>>::fetch().expect("state present");
        assert!(map.contains("a").unwrap(), "entry 'a' is present");
        assert_eq!(map.len().unwrap(), 2, "both entries enumerable");
        assert_eq!(
            map.range(..).unwrap().collect::<Vec<(String, String)>>(),
            vec![("b".to_owned(), "B".to_owned())],
            "control: with a matching marker the O(1) range() serves the stale subset"
        );
    });

    with_runtime_env(env_for(&state, [1u8; 32]), || {
        replay_child_links::<MainStorage>(&delta);
    });

    with_runtime_env(env_for(&state, [1u8; 32]), || {
        let map = Root::<SortedMap<String, String, MainStorage>>::fetch().expect("state present");
        let ordered: Vec<(String, String)> = map.range(..).unwrap().collect();
        assert_eq!(
            ordered,
            vec![
                ("a".to_owned(), "A".to_owned()),
                ("b".to_owned(), "B".to_owned())
            ],
            "index-backed range() still served a stale subset — the sync/apply \
             path did not invalidate the ordered-index marker (sdk-js#87)"
        );
        assert_eq!(map.first().unwrap(), Some(("a".to_owned(), "A".to_owned())));
        assert_eq!(map.last().unwrap(), Some(("b".to_owned(), "B".to_owned())));
    });
}

// === #3333 REOPENED: a post-sync LOCAL insert must not re-validate a stale index ===
//
// #3323 made the marker node-local; the apply-path clear (above) heals a peer
// whose index was left stale by a synced child link. Both fixes assume the ONLY
// thing that stamps the marker *valid* is a mutation from an already-consistent
// index (or a rebuild). But `insert`/`remove` maintain the ordered index
// INCREMENTALLY (one `index_put`/`index_remove`) and then stamp the marker to the
// collection's *current* `full_hash` — which already reflects any children a
// prior sync-apply linked. So this exact interleaving reintroduces the false
// positive that #3333 is about (surfaced JS-path-first in sdk-js#87, because the
// JS write path applies the peer's element before the local write commits):
//
//   1. sync delivers converged children `{a}` but NOT the node-local index
//      (index fresh/empty, marker absent — the "second node" state above).
//   2. a LOCAL `insert("b")` runs: children become `{a,b}`, `index_put("b")`
//      writes ONLY `b`, and `stamp_index_marker()` records `marker = H({a,b})`.
//   3. the index now holds `{b}` while the marker equals the full-set hash, so
//      `index_marker_current()` is `true` and the next ordered read SKIPS the
//      rebuild and serves the stale subset `{b}` forever.
//
// This is the SAME observable state the apply tests manufacture with
// `drop_sorted_index_entry_for_testing`, but reached through a legitimate code
// path (sync + local write), so it is the real regression. `contains`/`len`
// read children directly and converge; only the ordered readers diverge.
//
// The fix: `insert`/`remove` must only maintain the index incrementally + stamp
// when the index was ALREADY consistent (marker current) *before* the mutation.
// When a sync left it stale, they leave the marker stale so the next ordered
// read rebuilds from the full child set — the invariant `ensure_index`'s own doc
// comment already promises ("the local insert path likewise leaves the marker
// stale, so both mutation paths funnel back through this one rebuild").

/// SortedSet: after sync delivers converged children to a node with a fresh
/// ordered index, a LOCAL insert of a new element must not stamp a valid marker
/// over the still-unbuilt index — the next ordered read must still see the full,
/// converged set (not just the locally-inserted element).
#[test]
fn sorted_set_local_insert_after_sync_does_not_hide_synced_elements() {
    reset_environment();
    let state: SharedState = Rc::new(RefCell::new(HashMap::new()));

    // Node A builds + commits the converged element `a` (warms the shared index
    // mock as a side effect of `insert`).
    with_runtime_env(env_for(&state, [1u8; 32]), || {
        let mut set = Root::new(SortedSet::<String, MainStorage>::new);
        assert!(set.insert("a".to_owned()).unwrap());
        set.commit();
    });

    // Node B shares node A's converged state but has a FRESH node-local index
    // (sync never ships the index) — the "second node" state.
    clear_sorted_index_for_testing();

    // Node B now performs a LOCAL insert of a NEW element `b` BEFORE any ordered
    // read has rebuilt its index. Pre-fix this incrementally indexes only `b` and
    // stamps `marker = H({a,b})`, hiding the synced `a` from ordered reads.
    with_runtime_env(env_for(&state, [2u8; 32]), || {
        let mut set = Root::<SortedSet<String, MainStorage>>::fetch().expect("node B sees state");
        assert!(set.insert("b".to_owned()).unwrap());
        set.commit();
    });

    // The next ordered read on node B must converge to the FULL set. Pre-fix it
    // returns just `["b"]` (marker current over a `{b}`-only index).
    with_runtime_env(env_for(&state, [2u8; 32]), || {
        let set = Root::<SortedSet<String, MainStorage>>::fetch().expect("state present");
        // Sanity: membership + count converge (they read children directly).
        assert!(set.contains("a").unwrap(), "synced child 'a' present");
        assert!(set.contains("b").unwrap(), "locally-inserted 'b' present");
        assert_eq!(set.len().unwrap(), 2, "both children enumerable");

        let got: Vec<String> = set.iter().unwrap().collect();
        assert_eq!(
            got,
            vec!["a".to_owned(), "b".to_owned()],
            "ordered iter() served a stale subset: a local insert after sync \
             re-stamped the ordered-index marker over a fresh index, hiding the \
             synced element (core#3333)"
        );
        assert_eq!(set.first().unwrap(), Some("a".to_owned()));
        assert_eq!(set.last().unwrap(), Some("b".to_owned()));
    });
}

/// SortedMap: same shape via the key/value API. A local `insert` of a new key
/// after sync must not hide synced keys from the index-backed ordered readers.
#[test]
fn sorted_map_local_insert_after_sync_does_not_hide_synced_entries() {
    reset_environment();
    let state: SharedState = Rc::new(RefCell::new(HashMap::new()));

    with_runtime_env(env_for(&state, [1u8; 32]), || {
        let mut map = Root::new(SortedMap::<String, String, MainStorage>::new);
        assert!(map
            .insert("a".to_owned(), "A".to_owned())
            .unwrap()
            .is_none());
        map.commit();
    });

    clear_sorted_index_for_testing();

    with_runtime_env(env_for(&state, [2u8; 32]), || {
        let mut map =
            Root::<SortedMap<String, String, MainStorage>>::fetch().expect("node B sees state");
        assert!(map
            .insert("b".to_owned(), "B".to_owned())
            .unwrap()
            .is_none());
        map.commit();
    });

    with_runtime_env(env_for(&state, [2u8; 32]), || {
        let map = Root::<SortedMap<String, String, MainStorage>>::fetch().expect("state present");
        assert!(map.contains("a").unwrap(), "synced entry 'a' present");
        assert!(map.contains("b").unwrap(), "locally-inserted 'b' present");
        assert_eq!(map.len().unwrap(), 2, "both entries enumerable");

        let ordered: Vec<(String, String)> = map.range(..).unwrap().collect();
        assert_eq!(
            ordered,
            vec![
                ("a".to_owned(), "A".to_owned()),
                ("b".to_owned(), "B".to_owned())
            ],
            "index-backed range() served a stale subset: a local insert after \
             sync re-stamped the ordered-index marker over a fresh index, hiding \
             the synced entry (core#3333)"
        );
        assert_eq!(map.first().unwrap(), Some(("a".to_owned(), "A".to_owned())));
        assert_eq!(map.last().unwrap(), Some(("b".to_owned(), "B".to_owned())));
    });
}
