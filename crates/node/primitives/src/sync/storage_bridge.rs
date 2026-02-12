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
use calimero_primitives::crdt::CrdtType;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::env::RuntimeEnv;
use calimero_storage::index::Index;
use calimero_storage::interface::Interface;
use calimero_storage::store::{Key, MainStorage};
use calimero_store::{Store, key, types};
use eyre::Result;
use tracing::warn;

use super::hash_comparison::{LeafMetadata, TreeLeafData, TreeNode};

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

/// Get a tree node from the local Merkle tree Index.
///
/// This is a shared helper used by both `HashComparisonProtocol` and `SyncManager`
/// to avoid code duplication. Must be called within a `with_runtime_env` context.
///
/// # Arguments
///
/// * `context_id` - The context being synchronized
/// * `node_id` - The ID of the node to look up
/// * `is_root_request` - If true, looks up the context root instead of `node_id`
///
/// # Returns
///
/// * `Ok(Some(TreeNode))` - The tree node (leaf or internal)
/// * `Ok(None)` - Node not found
/// * `Err(_)` - On storage errors
pub fn get_local_tree_node(
    context_id: ContextId,
    node_id: &[u8; 32],
    is_root_request: bool,
) -> Result<Option<TreeNode>> {
    // Determine the entity ID to look up
    let entity_id = if is_root_request {
        // For root request, look up the context root
        Id::new(*context_id.as_ref())
    } else {
        // For child requests, node_id IS the entity ID
        Id::new(*node_id)
    };

    // Get the entity's index from the Merkle tree
    let index = match Index::<MainStorage>::get_index(entity_id) {
        Ok(Some(idx)) => idx,
        Ok(None) => return Ok(None),
        Err(e) => {
            warn!(
                %context_id,
                %entity_id,
                error = %e,
                "Failed to get index for entity"
            );
            return Ok(None);
        }
    };

    // Get the full hash from the index
    let full_hash = index.full_hash();

    // Get children IDs from the index
    let children_ids: Vec<[u8; 32]> = index
        .children()
        .map(|children| {
            children
                .iter()
                .map(|child| *child.id().as_bytes())
                .collect()
        })
        .unwrap_or_default();

    // Determine if this is a leaf or internal node
    if children_ids.is_empty() {
        // Leaf node - try to get entity data
        if let Some(entry_data) = Interface::<MainStorage>::find_by_id_raw(entity_id) {
            let metadata = LeafMetadata::new(
                // Get CRDT type from index metadata if available
                index
                    .metadata
                    .crdt_type
                    .clone()
                    .unwrap_or(CrdtType::LwwRegister),
                index.metadata.updated_at(),
                // Collection ID - use parent if available
                [0u8; 32],
            );

            let leaf_data = TreeLeafData::new(*entity_id.as_bytes(), entry_data, metadata);

            Ok(Some(TreeNode::leaf(
                *entity_id.as_bytes(),
                full_hash,
                leaf_data,
            )))
        } else {
            // Index exists but no entry data - treat as internal node with no children
            // This can happen for collection containers
            Ok(Some(TreeNode::internal(
                *entity_id.as_bytes(),
                full_hash,
                vec![],
            )))
        }
    } else {
        // Internal node with children
        Ok(Some(TreeNode::internal(
            *entity_id.as_bytes(),
            full_hash,
            children_ids,
        )))
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
