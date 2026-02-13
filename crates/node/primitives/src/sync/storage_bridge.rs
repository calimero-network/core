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
}
