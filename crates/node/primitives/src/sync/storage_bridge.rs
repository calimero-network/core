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
//! let runtime_env = create_runtime_env(&store, context_id, identity);
//!
//! // Use with storage APIs
//! with_runtime_env(runtime_env, || {
//!     let index = Index::<MainStorage>::get_index(entity_id)?;
//!     // ...
//! });
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::env::RuntimeEnv;
use calimero_storage::store::Key;
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
/// * `executor_id` - The identity executing operations
///
/// # Example
///
/// ```ignore
/// let env = create_runtime_env(&store, context_id, identity);
/// let result = with_runtime_env(env, || {
///     Index::<MainStorage>::get_index(entity_id)
/// });
/// ```
pub fn create_runtime_env(
    store: &Store,
    context_id: ContextId,
    executor_id: PublicKey,
) -> RuntimeEnv {
    let callbacks = create_storage_callbacks(store, context_id);
    RuntimeEnv::new(
        callbacks.read,
        callbacks.write,
        callbacks.remove,
        *context_id.as_ref(),
        *executor_id.as_ref(),
    )
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
        let env = create_runtime_env(&store, context_id, identity);

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

        let env = create_runtime_env(&store, context_id, identity);

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
                ApplyContext {
                    causal_parents: &[],
                },
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
            eprintln!("Copied {} ContextState records", copied);
            assert!(copied > 0, "should have copied records");
        }

        // Read from store2 via bridge (like the HashComparison responder)
        let env2 = create_runtime_env(&store2, context_id, identity);
        let read_result2 = with_runtime_env(env2, || Index::<MainStorage>::get_index(root_id));
        eprintln!("Read from store2: {:?}", read_result2);
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
}
