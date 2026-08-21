//! Storage bridge utilities for sync protocols.
//!
//! This module provides helpers to bridge `calimero-storage` APIs (which use
//! the `RuntimeEnv` thread-local) to the underlying `calimero-store` backend.
//!
//! # Why This Exists
//!
//! The `calimero-storage` crate provides high-level storage APIs (`Index`, `Interface`)
//! that operate through a thread-local `RuntimeEnv`. This `RuntimeEnv` contains
//! callbacks that route read/write/remove operations to the actual database.
//!
//! This module provides a single, reusable way to create these callbacks from
//! a `Store`, regardless of the backend (RocksDB or InMemoryDB).
//!
//! # Usage
//!
//! ```ignore
//! use calimero_node_primitives::sync::storage_bridge::create_runtime_env;
//!
//! // Works with any Store backend (RocksDB or InMemoryDB)
//! let runtime_env = create_runtime_env(&store, context_id, identity, test_account());
//!
//! // Use with storage APIs
//! with_runtime_env(runtime_env, || {
//!     let index = Index::<MainStorage>::get_index(entity_id)?;
//!     // ...
//! });
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use calimero_account::AccountId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_primitives::utils::prefix_upper_bound;
use calimero_storage::env::{IndexCallbacks, RuntimeEnv};
use calimero_storage::store::Key;
use calimero_store::db::Column;
use calimero_store::{key, types, Store};
use tracing::warn;

/// Create a `RuntimeEnv` that bridges `calimero-storage` to a `Store`.
///
/// This is the canonical way to set up storage access for sync protocols.
/// The returned `RuntimeEnv` can be used with `with_runtime_env()` to enable
/// `Index<MainStorage>` and `Interface<MainStorage>` operations.
///
/// # Arguments
///
/// * `store` - The underlying store (works with both RocksDB and InMemoryDB)
/// * `context_id` - The context being accessed
/// * `executor_id` - The DEVICE identity executing operations (the replica id,
///   and what a signed write's `signature_data.signer` records)
/// * `account_id` - The ACCOUNT that device speaks for here (the principal both
///   a writer set and an owner stamp gate on). Callers in `calimero-node` get it
///   from `calimero_governance_store::account_for_context`, which is the same
///   resolution the execute path uses, so a native storage operation and a
///   guest one agree about who this node is.
///
/// Both are required, and passing the device for both is a bug rather than a
/// shortcut: gates and owner stamps read the account while the signer and the
/// CRDT internals underneath (a counter slot, an HLC seed) read the device, so
/// collapsing one into the other re-creates the "your second device is a
/// stranger" behaviour the account plane exists to remove — and silently,
/// because both are 32 bytes.
///
/// # Example
///
/// ```ignore
/// let account = calimero_governance_store::account_for_context(&store, &context_id)?;
/// let env = create_runtime_env(&store, context_id, identity, account);
/// let result = with_runtime_env(env, || {
///     Index::<MainStorage>::get_index(entity_id)
/// });
/// ```
pub fn create_runtime_env(
    store: &Store,
    context_id: ContextId,
    executor_id: PublicKey,
    account_id: AccountId,
) -> RuntimeEnv {
    let callbacks = create_storage_callbacks(store, context_id);
    RuntimeEnv::new(
        callbacks.read,
        callbacks.write,
        callbacks.remove,
        *context_id.as_ref(),
        *executor_id.as_ref(),
        *account_id.as_bytes(),
    )
    .with_index(create_index_callbacks(store, context_id))
}

/// Build ordered-index host callbacks bridging `calimero-storage`'s node-local
/// index + validity marker to this context's RocksDB `Column::SortedIndex` /
/// `Column::SortedIndexMeta`.
///
/// This is the sync-side twin of the runtime's `build_runtime_env` bridge: it
/// makes host-side `SortedSet`/`SortedMap` ops during native apply
/// (`Interface::apply_action` on the HashComparison / delta-apply path) target
/// the SAME durable, context-scoped columns the guest and JS read paths use —
/// so an apply-time marker clear (invalidate-on-sync) is seen by the next
/// ordered read instead of landing in a per-thread mock (sdk-js#87).
///
/// Keys mirror `ContextStorage::index_key`: the adaptor-level `collection ‖
/// order_key` (marker: `collection_id`) is prefixed with the 32-byte context id
/// to keep contexts disjoint in the shared columns.
fn create_index_callbacks(store: &Store, context_id: ContextId) -> IndexCallbacks {
    // `context ‖ key` — the column-scoped key for this context.
    fn scoped(ctx: &[u8; 32], key: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(ctx.len() + key.len());
        out.extend_from_slice(ctx);
        out.extend_from_slice(key);
        out
    }

    let ctx = *context_id.as_ref();

    let set = {
        let store = store.clone();
        Rc::new(move |key: &[u8], value: &[u8]| {
            store
                .raw_put(Column::SortedIndex, &scoped(&ctx, key), value)
                .is_ok()
        }) as Rc<dyn Fn(&[u8], &[u8]) -> bool>
    };
    let remove = {
        let store = store.clone();
        Rc::new(move |key: &[u8]| {
            store
                .raw_delete(Column::SortedIndex, &scoped(&ctx, key))
                .is_ok()
        }) as Rc<dyn Fn(&[u8]) -> bool>
    };
    let remove_prefix = {
        let store = store.clone();
        Rc::new(move |prefix: &[u8]| {
            let lo = scoped(&ctx, prefix);
            let hi = prefix_upper_bound(&lo);
            store
                .raw_delete_range(Column::SortedIndex, &lo, &hi)
                .is_ok()
        }) as Rc<dyn Fn(&[u8]) -> bool>
    };
    let scan = {
        let store = store.clone();
        Rc::new(
            move |lo: &[u8], hi: &[u8], offset: usize, limit: Option<usize>| {
                let full_lo = scoped(&ctx, lo);
                let full_hi = scoped(&ctx, hi);
                // Walk O(offset + limit), matching ContextStorage.
                let max = limit.map(|n| offset.saturating_add(n));
                let pairs = store
                    .raw_scan(Column::SortedIndex, &full_lo, &full_hi, max)
                    .unwrap_or_default();
                let stripped = pairs
                    .into_iter()
                    .filter_map(|(k, v)| k.get(ctx.len()..).map(|key| (key.to_vec(), v)))
                    .skip(offset);
                match limit {
                    Some(n) => stripped.take(n).collect(),
                    None => stripped.collect(),
                }
            },
        ) as Rc<dyn Fn(&[u8], &[u8], usize, Option<usize>) -> Vec<(Vec<u8>, Vec<u8>)>>
    };
    let last = {
        let store = store.clone();
        Rc::new(move |lo: &[u8], hi: &[u8]| {
            let full_lo = scoped(&ctx, lo);
            let full_hi = scoped(&ctx, hi);
            store
                .raw_last(Column::SortedIndex, &full_lo, &full_hi)
                .ok()
                .flatten()
                .and_then(|(k, v)| k.get(ctx.len()..).map(|key| (key.to_vec(), v)))
        }) as Rc<dyn Fn(&[u8], &[u8]) -> Option<(Vec<u8>, Vec<u8>)>>
    };
    let meta_set = {
        let store = store.clone();
        Rc::new(move |key: &[u8], value: &[u8]| {
            store
                .raw_put(Column::SortedIndexMeta, &scoped(&ctx, key), value)
                .is_ok()
        }) as Rc<dyn Fn(&[u8], &[u8]) -> bool>
    };
    let meta_get = {
        let store = store.clone();
        Rc::new(move |key: &[u8]| {
            store
                .raw_get(Column::SortedIndexMeta, &scoped(&ctx, key))
                .ok()
                .flatten()
        }) as Rc<dyn Fn(&[u8]) -> Option<Vec<u8>>>
    };
    let meta_clear = {
        let store = store.clone();
        Rc::new(move |key: &[u8]| {
            store
                .raw_delete(Column::SortedIndexMeta, &scoped(&ctx, key))
                .is_ok()
        }) as Rc<dyn Fn(&[u8]) -> bool>
    };

    IndexCallbacks {
        set,
        remove,
        remove_prefix,
        scan,
        last,
        meta_set,
        meta_get,
        meta_clear,
    }
}

/// Storage callback closures that bridge `calimero-storage` Key API to the Store.
///
/// These closures translate `calimero-storage::Key` (Index/Entry) to
/// `calimero-store::ContextStateKey` for access to the actual database.
#[expect(
    clippy::type_complexity,
    reason = "Matches RuntimeEnv callback signatures"
)]
struct StorageCallbacks {
    read: Rc<dyn Fn(&Key) -> Option<Vec<u8>>>,
    write: Rc<dyn Fn(Key, &[u8]) -> bool>,
    remove: Rc<dyn Fn(&Key) -> bool>,
}

/// Create storage callbacks for a context.
///
/// These bridge the `calimero-storage` Key-based API to the underlying
/// `calimero-store` ContextStateKey-based storage.
#[expect(
    clippy::type_complexity,
    reason = "Matches RuntimeEnv callback signatures"
)]
fn create_storage_callbacks(store: &Store, context_id: ContextId) -> StorageCallbacks {
    let read: Rc<dyn Fn(&Key) -> Option<Vec<u8>>> = {
        let handle = store.handle();
        let ctx_id = context_id;
        Rc::new(move |key: &Key| {
            let storage_key = key.to_bytes();
            let state_key = key::ContextState::new(ctx_id, storage_key);
            match handle.get(&state_key) {
                Ok(Some(state)) => Some(state.value.into_boxed().into_vec()),
                Ok(None) => None,
                Err(e) => {
                    warn!(
                        %ctx_id,
                        storage_key = %hex::encode(storage_key),
                        error = ?e,
                        "Storage read failed"
                    );
                    None
                }
            }
        })
    };

    let write: Rc<dyn Fn(Key, &[u8]) -> bool> = {
        let handle_cell: Rc<RefCell<_>> = Rc::new(RefCell::new(store.handle()));
        let ctx_id = context_id;
        Rc::new(move |key: Key, value: &[u8]| {
            let storage_key = key.to_bytes();
            let state_key = key::ContextState::new(ctx_id, storage_key);
            let slice: calimero_store::slice::Slice<'_> = value.to_vec().into();
            let state_value = types::ContextState::from(slice);
            handle_cell
                .borrow_mut()
                .put(&state_key, &state_value)
                .is_ok()
        })
    };

    let remove: Rc<dyn Fn(&Key) -> bool> = {
        let handle_cell: Rc<RefCell<_>> = Rc::new(RefCell::new(store.handle()));
        let ctx_id = context_id;
        Rc::new(move |key: &Key| {
            let storage_key = key.to_bytes();
            let state_key = key::ContextState::new(ctx_id, storage_key);
            handle_cell.borrow_mut().delete(&state_key).is_ok()
        })
    };

    StorageCallbacks {
        read,
        write,
        remove,
    }
}

#[cfg(test)]
mod tests {
    /// A stand-in account for these tests, deliberately unequal to the identity
    /// they pass as the device — nothing here is writer-set guarded, so the pair
    /// only has to be distinguishable.
    fn test_account() -> AccountId {
        AccountId::from([0xAC; 32])
    }

    use std::sync::Arc;

    use super::*;
    use calimero_storage::env::with_runtime_env;
    use calimero_storage::index::Index;
    use calimero_storage::store::MainStorage;
    use calimero_store::db::InMemoryDB;

    #[test]
    fn test_create_runtime_env_with_inmemory() {
        // Create an in-memory store
        let db = InMemoryDB::owned();
        let store = Store::new(Arc::new(db));

        // Create a context ID and identity
        let context_id = ContextId::from([1u8; 32]);
        let identity = PublicKey::from([2u8; 32]);

        // Create RuntimeEnv - should not panic
        let env = create_runtime_env(&store, context_id, identity, test_account());

        // Use it with storage APIs
        let result = with_runtime_env(env, || {
            // Try to get a non-existent index - should return None, not panic
            Index::<MainStorage>::get_index(calimero_storage::address::Id::root())
        });

        // Root index doesn't exist yet, should be Ok(None)
        assert!(result.is_ok());
    }

    /// Test: write an entity through the bridge, then read it back.
    ///
    /// Reproduces the production path:
    /// 1. Create RuntimeEnv via `create_runtime_env` (same as sync bridge)
    /// 2. Write an entity via `Interface::apply_action` (same as WASM runtime)
    /// 3. Read back via `Index::get_index` (same as HashComparison responder)
    #[test]
    fn test_write_and_read_entity_via_bridge() {
        use calimero_storage::address::Id;
        use calimero_storage::entities::Metadata;
        use calimero_storage::interface::{Action, ApplyContext, Interface};

        let db = InMemoryDB::owned();
        let store = Store::new(Arc::new(db));
        let context_id = ContextId::from([1u8; 32]);
        let identity = PublicKey::from([2u8; 32]);

        let env = create_runtime_env(&store, context_id, identity, test_account());

        // Write: create root entity
        let root_id = Id::new(*context_id.as_ref());
        let write_result = with_runtime_env(env.clone(), || {
            Interface::<MainStorage>::apply_action(
                Action::Update {
                    id: root_id,
                    data: vec![],
                    ancestors: vec![],
                    metadata: Metadata::default(),
                },
                &ApplyContext::empty(),
            )
        });
        assert!(write_result.is_ok(), "apply_action should succeed");

        // Read back: Index::get_index should find the root
        let read_result =
            with_runtime_env(env.clone(), || Index::<MainStorage>::get_index(root_id));
        assert!(read_result.is_ok(), "get_index should not error");
        assert!(
            read_result.unwrap().is_some(),
            "root entity should exist after apply_action"
        );

        // Verify root hash is non-zero
        let hash_result = with_runtime_env(env.clone(), || {
            Index::<MainStorage>::get_hashes_for(root_id)
        });
        assert!(hash_result.is_ok());
        let hashes = hash_result.unwrap();
        assert!(hashes.is_some(), "root should have hashes");

        // Now simulate snapshot: read raw ContextState, write to new store, read back
        let db2 = InMemoryDB::owned();
        let store2 = Store::new(Arc::new(db2));

        // Copy all ContextState records from store to store2 (like snapshot sync)
        {
            let src_handle = store.handle();
            let mut dst_handle = store2.handle();
            let mut copied = 0;
            let mut iter = src_handle
                .iter::<calimero_store::key::ContextState>()
                .unwrap();
            for (key_result, value_result) in iter.entries() {
                let key = key_result.unwrap();
                let value = value_result.unwrap();
                if key.context_id() == context_id {
                    let state_key = key.state_key();
                    let dst_key = calimero_store::key::ContextState::new(context_id, state_key);
                    let slice: calimero_store::slice::Slice<'_> = value.value.to_vec().into();
                    let dst_value = calimero_store::types::ContextState::from(slice);
                    dst_handle.put(&dst_key, &dst_value).unwrap();
                    copied += 1;
                }
            }
            eprintln!("Copied {copied} ContextState records");
            assert!(copied > 0, "should have copied records");
        }

        // Read from store2 via bridge (like the HashComparison responder)
        let env2 = create_runtime_env(&store2, context_id, identity, test_account());
        let read_result2 = with_runtime_env(env2, || Index::<MainStorage>::get_index(root_id));
        eprintln!("Read from store2: {read_result2:?}");
        assert!(
            read_result2.is_ok(),
            "get_index from snapshot-restored store should not error: {:?}",
            read_result2.err()
        );
        assert!(
            read_result2.unwrap().is_some(),
            "root entity should exist in snapshot-restored store"
        );
    }

    /// Host-side (JS-path) regression for sdk-js#87: a native `SortedSet` whose
    /// ordered index is stale but whose validity marker still matches the
    /// converged `full_hash` must self-heal when a synced child is applied.
    ///
    /// This exercises the REAL host store the JS SDK uses: with the index bridge
    /// installed by `create_runtime_env`, the ordered index + marker live in the
    /// RocksDB `Column::SortedIndex` / `Column::SortedIndexMeta` (context-scoped),
    /// NOT the storage crate's process-thread-local mock. The receiver applies a
    /// child via the same native `Interface::apply_action` path HashComparison
    /// uses; its `index_meta_clear` must invalidate the RocksDB marker so the
    /// next ordered read rebuilds. Fails if the bridge or the apply-time clear is
    /// removed (marker stays matched → stale subset served forever).
    #[test]
    fn sorted_set_apply_invalidates_host_index_marker() {
        use calimero_storage::collections::{Root, SortedSet};
        use calimero_storage::delta::StorageDelta;
        use calimero_storage::env::take_last_artifact;
        use calimero_storage::interface::{ApplyContext, Interface};

        let db = InMemoryDB::owned();
        let store = Store::new(Arc::new(db));
        let context_id = ContextId::from([9u8; 32]);
        let identity = PublicKey::from([2u8; 32]);
        let ctx = *context_id.as_ref();

        // Build the set host-side through the bridge → index + marker land in the
        // RocksDB SortedIndex / SortedIndexMeta columns. Capture the delta a peer
        // would apply.
        let delta = with_runtime_env(
            create_runtime_env(&store, context_id, identity, test_account()),
            || {
                let mut set = Root::new(SortedSet::<String, MainStorage>::new);
                assert!(set.insert("a".to_owned()).unwrap());
                assert!(set.insert("b".to_owned()).unwrap());
                // Warm the ordered index (rebuild + stamp marker) in RocksDB.
                assert_eq!(
                    set.iter().unwrap().collect::<Vec<_>>(),
                    vec!["a".to_owned(), "b".to_owned()]
                );
                set.commit();
                take_last_artifact().expect("commit emitted a delta")
            },
        );

        // Sanity: the ordered index really is in RocksDB (bridge is wired), not
        // the mock — there is at least one SortedIndex row for this context.
        let index_rows = store
            .raw_scan(Column::SortedIndex, &ctx, &prefix_upper_bound(&ctx), None)
            .unwrap();
        assert!(
            !index_rows.is_empty(),
            "ordered index must be backed by RocksDB SortedIndex (bridge wired)"
        );

        // Manufacture the false positive: wipe the SortedIndex rows for this
        // context (index is now a strict subset — empty) but LEAVE the marker in
        // SortedIndexMeta intact, so `index_marker_current()` still returns true.
        store
            .raw_delete_range(Column::SortedIndex, &ctx, &prefix_upper_bound(&ctx))
            .unwrap();
        assert!(
            !store
                .raw_scan(
                    Column::SortedIndexMeta,
                    &ctx,
                    &prefix_upper_bound(&ctx),
                    None
                )
                .unwrap()
                .is_empty(),
            "marker must survive the index wipe (this is the false-positive state)"
        );

        // CONTROL: the O(1) marker-only read trusts the matching marker and
        // serves the stale (empty) index — the bug.
        with_runtime_env(
            create_runtime_env(&store, context_id, identity, test_account()),
            || {
                let set = Root::<SortedSet<String, MainStorage>>::fetch().expect("state present");
                assert!(
                    set.contains("a").unwrap(),
                    "child 'a' still present in state"
                );
                assert_eq!(set.len().unwrap(), 2, "both children enumerable");
                assert_eq!(
                    set.iter().unwrap().collect::<Vec<String>>(),
                    Vec::<String>::new(),
                    "control: matching marker → stale (empty) ordered index served"
                );
            },
        );

        // Drive the REAL receiver apply path: replay the collection's own child
        // links through native apply_action (idempotent → full_hash unchanged, so
        // only the marker clear can heal the read).
        let actions = match borsh::from_slice::<StorageDelta>(&delta).expect("decode delta") {
            StorageDelta::Actions(a) => a,
            StorageDelta::CausalActions { actions, .. } => actions,
        };
        with_runtime_env(
            create_runtime_env(&store, context_id, identity, test_account()),
            || {
                for action in actions {
                    if action.id().is_root() {
                        continue;
                    }
                    Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
                        .expect("apply_action");
                }
            },
        );

        // The apply cleared the RocksDB marker, so the next ordered read rebuilds
        // and serves the full converged set.
        with_runtime_env(
            create_runtime_env(&store, context_id, identity, test_account()),
            || {
                let set = Root::<SortedSet<String, MainStorage>>::fetch().expect("state present");
                assert_eq!(
                    set.iter().unwrap().collect::<Vec<String>>(),
                    vec!["a".to_owned(), "b".to_owned()],
                    "host-side ordered iter() stayed stale — the native apply did not \
                 invalidate the RocksDB SortedIndexMeta marker (sdk-js#87)"
                );
                assert_eq!(set.first().unwrap(), Some("a".to_owned()));
                assert_eq!(set.last().unwrap(), Some("b".to_owned()));
            },
        );
    }

    /// TRUE concurrent-writer reproduction for core#3333 on the REAL RocksDB
    /// `ContextStorage` (not the runtime `InMemoryStorage` the conformance
    /// harness uses, and not the storage-crate thread-local mock that #3323/#3328
    /// were validated against).
    ///
    /// Node A inserts "a" and warms its ordered index; node B inserts "b" and
    /// warms its own. Then node B applies node A's delta child actions through
    /// the same native `Interface::apply_action` path a real receiver uses. The
    /// element set converges ({a,b}); the ordered `iter()` must too. A failure
    /// here is #3333 reproduced on the real host store.
    ///
    /// Ignored in the default suite: it constructs `Root::new(...)` under a
    /// `create_runtime_env` thread-local without a context/`ROOT_ID` init, which
    /// is fine run in isolation but orphans (`CannotCreateOrphan`) under the
    /// full parallel `cargo test` where the thread-local storage env is shared.
    /// The faithful, maintained reproduction of this layer is
    /// `crates/node/tests/sorted_index_hc_merge.rs`; run this one directly with
    /// `--ignored --test-threads=1` for ad-hoc storage-layer inspection.
    #[test]
    #[ignore = "thread-local storage-env isolation; superseded by tests/sorted_index_hc_merge.rs (#3333)"]
    fn sorted_set_concurrent_writers_ordered_read_real_store() {
        use calimero_storage::collections::{Root, SortedSet};
        use calimero_storage::delta::StorageDelta;
        use calimero_storage::env::take_last_artifact;
        use calimero_storage::interface::{ApplyContext, Interface};

        let context_id = ContextId::from([42u8; 32]);
        let identity = PublicKey::from([2u8; 32]);

        // Node A: insert "a", warm the ordered index, capture the delta.
        let store_a = Store::new(Arc::new(InMemoryDB::owned()));
        let delta_a = with_runtime_env(
            create_runtime_env(&store_a, context_id, identity, test_account()),
            || {
                let mut set = Root::new(SortedSet::<String, MainStorage>::new);
                // Pin the collection to the deterministic field-name id (what the
                // `#[app::state]` macro does after `init`) so both nodes address the
                // SAME collection — otherwise each `Root::new` mints a random id and
                // node A's element can never merge into node B's set.
                set.reassign_deterministic_id("tags");
                assert!(set.insert("a".to_owned()).unwrap());
                assert_eq!(
                    set.iter().unwrap().collect::<Vec<_>>(),
                    vec!["a".to_owned()]
                );
                set.commit();
                take_last_artifact().expect("commit emitted a delta")
            },
        );

        // Node B: insert "b", warm its own ordered index (index={b}, marker=H_b).
        let store_b = Store::new(Arc::new(InMemoryDB::owned()));
        with_runtime_env(
            create_runtime_env(&store_b, context_id, identity, test_account()),
            || {
                let mut set = Root::new(SortedSet::<String, MainStorage>::new);
                set.reassign_deterministic_id("tags");
                assert!(set.insert("b".to_owned()).unwrap());
                assert_eq!(
                    set.iter().unwrap().collect::<Vec<_>>(),
                    vec!["b".to_owned()]
                );
                set.commit();
            },
        );

        // Node B applies node A's child actions (the "a" element + its ancestors)
        // via the real receiver apply path.
        let actions = match borsh::from_slice::<StorageDelta>(&delta_a).expect("decode delta") {
            StorageDelta::Actions(a) => a,
            StorageDelta::CausalActions { actions, .. } => actions,
        };
        with_runtime_env(
            create_runtime_env(&store_b, context_id, identity, test_account()),
            || {
                for action in actions {
                    if action.id().is_root() {
                        continue;
                    }
                    Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
                        .expect("apply_action");
                }
            },
        );

        // Node B: element set converged, and the ordered read must too.
        with_runtime_env(
            create_runtime_env(&store_b, context_id, identity, test_account()),
            || {
                let set = Root::<SortedSet<String, MainStorage>>::fetch().expect("state present");
                assert!(set.contains("a").unwrap(), "membership converged: 'a'");
                assert!(set.contains("b").unwrap(), "membership converged: 'b'");
                assert_eq!(set.len().unwrap(), 2, "both elements enumerable");
                assert_eq!(
                    set.iter().unwrap().collect::<Vec<String>>(),
                    vec!["a".to_owned(), "b".to_owned()],
                    "ordered iter() diverged after concurrent writes (core#3333)"
                );
            },
        );
    }
}
