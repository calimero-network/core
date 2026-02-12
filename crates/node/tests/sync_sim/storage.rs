//! Storage infrastructure for SimNode.
//!
//! See [Simulation Framework Spec](https://github.com/calimero-network/specs/blob/main/sync/simulation-framework.md):
//! - §5: In-memory Storage + DAG backends
//! - §7: State Digest and Hashing (Canonical State Digest)
//! - §11: HashComparison Protocol (MerkleAccurate Traversal)
//!
//! Provides an in-memory storage backend that uses the real Merkle tree
//! implementation from `calimero-storage`, enabling accurate simulation
//! of sync protocols that depend on tree structure (e.g., HashComparison).
//!
//! # Architecture (Spec §2)
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                          SimNode                                 │
//! │  ┌─────────────────────────────────────────────────────────────┐│
//! │  │                    SimStorage                                ││
//! │  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐         ││
//! │  │  │   Store     │  │ RuntimeEnv  │  │ Index<Main> │         ││
//! │  │  │ (InMemory)  │◄─┤ (callbacks) │◄─┤   APIs      │         ││
//! │  │  └─────────────┘  └─────────────┘  └─────────────┘         ││
//! │  └─────────────────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! This setup mirrors production where:
//! - `Store` backed by RocksDB → here backed by `InMemoryDB`
//! - `RuntimeEnv` callbacks route storage operations
//! - `Index<MainStorage>` provides Merkle tree operations
//!
//! # Why Real Storage?
//!
//! The spec (§11.1) emphasizes `MerkleAccurate` mode for HashComparison testing:
//! > Strategy: Recursive subtree comparison with real tree traversal.
//!
//! Using the real `calimero-storage` implementation ensures:
//! - Accurate hash propagation through tree hierarchy
//! - Correct subtree comparisons during HashComparison protocol
//! - Realistic entity count and depth calculations for protocol selection

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::{ChildInfo, Metadata};
use calimero_storage::env::{with_runtime_env, RuntimeEnv};
use calimero_storage::index::{EntityIndex, Index};
use calimero_storage::interface::{Action, Interface};
use calimero_storage::store::{Key, MainStorage};
use calimero_store::db::InMemoryDB;
use calimero_store::{key, types, Store};

/// In-memory storage for simulation.
///
/// Wraps a `calimero_store::Store` backed by `InMemoryDB` and provides
/// the bridging necessary to use `calimero_storage::Index<MainStorage>`.
#[derive(Clone)]
pub struct SimStorage {
    /// The underlying store (in-memory).
    store: Store,
    /// Context ID for this storage instance.
    context_id: ContextId,
    /// Executor ID (simulated node identity).
    executor_id: PublicKey,
}

impl std::fmt::Debug for SimStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimStorage")
            .field("context_id", &self.context_id)
            .field("executor_id", &self.executor_id)
            .finish_non_exhaustive()
    }
}

impl SimStorage {
    /// Create a new in-memory storage instance.
    pub fn new(context_id: ContextId, executor_id: PublicKey) -> Self {
        let db = InMemoryDB::owned();
        let store = Store::new(Arc::new(db));
        Self {
            store,
            context_id,
            executor_id,
        }
    }

    /// Execute a closure with the RuntimeEnv configured for this storage.
    ///
    /// This allows using `Index<MainStorage>` APIs which route through
    /// the RuntimeEnv callbacks to our in-memory store.
    pub fn with_index<R>(&self, f: impl FnOnce() -> R) -> R {
        let env = self.create_runtime_env();
        with_runtime_env(env, f)
    }

    /// Get the context ID.
    pub fn context_id(&self) -> ContextId {
        self.context_id
    }

    /// Get the root entity ID for this storage.
    ///
    /// The root ID is derived from the context ID and must be used
    /// instead of `Id::root()` when outside of `with_index()` scope.
    pub fn root_id(&self) -> Id {
        Id::new(*self.context_id.as_ref())
    }

    /// Create RuntimeEnv callbacks that route to our Store.
    ///
    /// Uses the same pattern as `hash_comparison.rs` to bridge
    /// `calimero-storage::Key` to `calimero-store::key::ContextState`.
    fn create_runtime_env(&self) -> RuntimeEnv {
        let store = self.store.clone();
        let context_id = self.context_id;

        // Read callback - converts storage Key to ContextState key and reads
        let read: Rc<dyn Fn(&Key) -> Option<Vec<u8>>> = {
            let handle = store.handle();
            let ctx_id = context_id;
            Rc::new(move |key: &Key| {
                let storage_key = key.to_bytes();
                let state_key = key::ContextState::new(ctx_id, storage_key);
                match handle.get(&state_key) {
                    Ok(Some(state)) => Some(state.value.into_boxed().into_vec()),
                    Ok(None) => None,
                    Err(_) => None,
                }
            })
        };

        // Write callback - converts storage Key to ContextState and writes
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

        // Remove callback - deletes from ContextState
        let remove: Rc<dyn Fn(&Key) -> bool> = {
            let handle_cell: Rc<RefCell<_>> = Rc::new(RefCell::new(store.handle()));
            let ctx_id = context_id;
            Rc::new(move |key: &Key| {
                let storage_key = key.to_bytes();
                let state_key = key::ContextState::new(ctx_id, storage_key);
                handle_cell.borrow_mut().delete(&state_key).is_ok()
            })
        };

        // Convert ContextId and PublicKey to [u8; 32] using AsRef
        let context_bytes: [u8; 32] = *self.context_id.as_ref();
        let executor_bytes: [u8; 32] = *self.executor_id.as_ref();

        RuntimeEnv::new(read, write, remove, context_bytes, executor_bytes)
    }

    // =========================================================================
    // Merkle Tree Operations
    // =========================================================================

    /// Get the root hash of the Merkle tree.
    ///
    /// The root ID is derived from the context ID.
    pub fn root_hash(&self) -> [u8; 32] {
        let root_id = self.root_id();
        self.with_index(|| {
            Index::<MainStorage>::get_hashes_for(root_id)
                .ok()
                .flatten()
                .map(|(full_hash, _own_hash)| full_hash)
                .unwrap_or([0; 32])
        })
    }

    /// Get entity index by ID.
    pub fn get_index(&self, id: Id) -> Option<EntityIndex> {
        self.with_index(|| Index::<MainStorage>::get_index(id).ok().flatten())
    }

    /// Get children of an entity.
    pub fn get_children(&self, parent_id: Id) -> Vec<ChildInfo> {
        self.with_index(|| Index::<MainStorage>::get_children_of(parent_id).unwrap_or_default())
    }

    /// Get hashes for an entity: (full_hash, own_hash).
    pub fn get_hashes(&self, id: Id) -> Option<([u8; 32], [u8; 32])> {
        self.with_index(|| Index::<MainStorage>::get_hashes_for(id).ok().flatten())
    }

    /// Check if an entity exists.
    pub fn has_entity(&self, id: Id) -> bool {
        self.get_index(id).is_some()
    }

    /// Get entity count (traverses the tree).
    pub fn entity_count(&self) -> usize {
        let root_id = self.root_id();
        self.with_index(|| self.count_entities_recursive(root_id))
    }

    /// Recursively count entities in the tree.
    fn count_entities_recursive(&self, id: Id) -> usize {
        let index = match Index::<MainStorage>::get_index(id).ok().flatten() {
            Some(idx) => idx,
            None => return 0,
        };

        let mut count = 1; // Count this entity
        if let Some(children) = index.children() {
            for child in children {
                count += self.count_entities_recursive(child.id());
            }
        }
        count
    }

    /// Check if the tree is empty (no root or root has no data).
    pub fn is_empty(&self) -> bool {
        let root_id = self.root_id();
        self.with_index(|| {
            Index::<MainStorage>::get_index(root_id)
                .ok()
                .flatten()
                .is_none()
        })
    }

    /// Get the maximum depth of the tree.
    ///
    /// Returns the longest path from root to any leaf.
    /// Empty tree returns 0, root-only returns 1.
    pub fn max_depth(&self) -> u32 {
        let root_id = self.root_id();
        self.with_index(|| self.compute_depth_recursive(root_id))
    }

    /// Recursively compute depth of the tree.
    fn compute_depth_recursive(&self, id: Id) -> u32 {
        let index = match Index::<MainStorage>::get_index(id).ok().flatten() {
            Some(idx) => idx,
            None => return 0,
        };

        let children = index.children();
        if children.is_none() || children.as_ref().map_or(true, |c| c.is_empty()) {
            // Leaf node
            return 1;
        }

        // Internal node: 1 + max depth of children
        let max_child_depth = children
            .unwrap()
            .iter()
            .map(|child| self.compute_depth_recursive(child.id()))
            .max()
            .unwrap_or(0);

        1 + max_child_depth
    }

    // =========================================================================
    // Entity Manipulation via Interface::apply_action (public API)
    // =========================================================================

    /// Initialize the root entity.
    ///
    /// Must be called before adding children. The root ID is derived from context_id.
    pub fn init_root(&self) {
        let root_id = self.root_id();
        self.with_index(|| {
            let action = Action::Update {
                id: root_id,
                data: vec![],
                ancestors: vec![],
                metadata: Metadata::default(),
            };
            let _ = Interface::<MainStorage>::apply_action(action);
        });
    }

    /// Add an entity with a parent relationship.
    ///
    /// Uses `Interface::apply_action(Action::Update)` which is the public API
    /// for creating/updating entities in the Merkle tree.
    pub fn add_entity_with_parent(&self, id: Id, parent_id: Id, data: &[u8], metadata: Metadata) {
        self.with_index(|| {
            // Get parent info for ancestors chain
            let parent_hash = Index::<MainStorage>::get_hashes_for(parent_id)
                .ok()
                .flatten()
                .map(|(full, _)| full)
                .unwrap_or([0; 32]);

            let parent_metadata = Index::<MainStorage>::get_index(parent_id)
                .ok()
                .flatten()
                .map(|idx| idx.metadata.clone())
                .unwrap_or_default();

            let ancestor = ChildInfo::new(parent_id, parent_hash, parent_metadata);

            let action = Action::Update {
                id,
                data: data.to_vec(),
                ancestors: vec![ancestor],
                metadata,
            };
            let _ = Interface::<MainStorage>::apply_action(action);
        });
    }

    /// Add a root-level entity (direct child of root).
    pub fn add_entity(&self, id: Id, data: &[u8], metadata: Metadata) {
        // Ensure root exists
        if self.is_empty() {
            self.init_root();
        }
        self.add_entity_with_parent(id, self.root_id(), data, metadata);
    }

    /// Get entity data by ID.
    pub fn get_entity_data(&self, id: Id) -> Option<Vec<u8>> {
        // Use with_index to ensure RuntimeEnv is set up, then read via callback
        self.with_index(|| {
            // MainStorage uses the RuntimeEnv callbacks to read
            use calimero_storage::store::StorageAdaptor;
            MainStorage::storage_read(Key::Entry(id))
        })
    }

    /// Remove an entity by marking it as deleted (creates tombstone).
    pub fn remove_entity(&self, id: Id) {
        self.with_index(|| {
            if let Some(index) = Index::<MainStorage>::get_index(id).ok().flatten() {
                let action = Action::DeleteRef {
                    id,
                    deleted_at: calimero_storage::env::time_now(),
                    metadata: index.metadata.clone(),
                };
                let _ = Interface::<MainStorage>::apply_action(action);
            }
        });
    }

    /// Check if an entity is deleted (tombstone).
    pub fn is_deleted(&self, id: Id) -> bool {
        self.with_index(|| Index::<MainStorage>::is_deleted(id).unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context_id() -> ContextId {
        ContextId::from([1u8; 32])
    }

    fn test_executor_id() -> PublicKey {
        PublicKey::from([2u8; 32])
    }

    #[test]
    fn test_storage_creation() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());
        assert!(storage.is_empty());
        assert_eq!(storage.root_hash(), [0; 32]);
    }

    #[test]
    fn test_init_root() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());
        storage.init_root();
        assert!(!storage.is_empty());
    }

    #[test]
    fn test_add_entity() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());

        let id = Id::new([10u8; 32]);
        storage.add_entity(id, b"hello world", Metadata::default());

        assert!(storage.has_entity(id));
        assert_eq!(storage.get_entity_data(id), Some(b"hello world".to_vec()));
    }

    #[test]
    fn test_root_hash_changes() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());

        let hash1 = storage.root_hash();

        let id = Id::new([10u8; 32]);
        storage.add_entity(id, b"hello", Metadata::default());

        let hash2 = storage.root_hash();
        assert_ne!(hash1, hash2, "Root hash should change after adding entity");
    }

    #[test]
    fn test_entity_count() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());

        // Initially empty (no root)
        assert_eq!(storage.entity_count(), 0);

        // Add root
        storage.init_root();
        assert_eq!(storage.entity_count(), 1);

        // Add child
        let id1 = Id::new([10u8; 32]);
        storage.add_entity_with_parent(id1, storage.root_id(), b"data1", Metadata::default());
        assert_eq!(storage.entity_count(), 2);

        // Add another child
        let id2 = Id::new([20u8; 32]);
        storage.add_entity_with_parent(id2, storage.root_id(), b"data2", Metadata::default());
        assert_eq!(storage.entity_count(), 3);
    }

    #[test]
    fn test_tree_structure() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());
        storage.init_root();

        // Add parent entity
        let parent_id = Id::new([10u8; 32]);
        storage.add_entity_with_parent(
            parent_id,
            storage.root_id(),
            b"parent",
            Metadata::default(),
        );

        // Add child under parent
        let child_id = Id::new([20u8; 32]);
        storage.add_entity_with_parent(child_id, parent_id, b"child", Metadata::default());

        // Verify structure
        let root_children = storage.get_children(storage.root_id());
        assert_eq!(root_children.len(), 1);
        assert_eq!(root_children[0].id(), parent_id);

        let parent_children = storage.get_children(parent_id);
        assert_eq!(parent_children.len(), 1);
        assert_eq!(parent_children[0].id(), child_id);

        // Verify parent relationship
        let child_index = storage.get_index(child_id).unwrap();
        assert_eq!(child_index.parent_id(), Some(parent_id));
    }

    #[test]
    fn test_hash_propagation() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());
        storage.init_root();

        let parent_id = Id::new([10u8; 32]);
        storage.add_entity_with_parent(
            parent_id,
            storage.root_id(),
            b"parent",
            Metadata::default(),
        );

        let hash_before = storage.root_hash();

        // Add child - should propagate hash change to root
        let child_id = Id::new([20u8; 32]);
        storage.add_entity_with_parent(child_id, parent_id, b"child", Metadata::default());

        let hash_after = storage.root_hash();
        assert_ne!(
            hash_before, hash_after,
            "Adding child should change root hash"
        );
    }

    #[test]
    fn test_multiple_storage_instances_isolated() {
        let ctx1 = ContextId::from([1u8; 32]);
        let ctx2 = ContextId::from([2u8; 32]);
        let exec = PublicKey::from([3u8; 32]);

        let storage1 = SimStorage::new(ctx1, exec);
        let storage2 = SimStorage::new(ctx2, exec);

        // Add entity to storage1 only
        let id = Id::new([10u8; 32]);
        storage1.add_entity(id, b"hello", Metadata::default());

        // Verify isolation
        assert!(storage1.has_entity(id));
        assert!(!storage2.has_entity(id));
    }

    #[test]
    fn test_max_depth_empty() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());
        assert_eq!(storage.max_depth(), 0, "Empty tree should have depth 0");
    }

    #[test]
    fn test_max_depth_root_only() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());
        storage.init_root();
        assert_eq!(storage.max_depth(), 1, "Root-only tree should have depth 1");
    }

    #[test]
    fn test_max_depth_shallow() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());
        storage.init_root();

        // Add 3 children directly under root (depth = 2)
        for i in 0..3 {
            let id = Id::new([10 + i; 32]);
            storage.add_entity_with_parent(id, storage.root_id(), b"leaf", Metadata::default());
        }

        assert_eq!(storage.max_depth(), 2, "Root + leaves should have depth 2");
    }

    #[test]
    fn test_max_depth_deep() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());
        storage.init_root();

        // Create a chain: root -> a -> b -> c -> d (depth = 5)
        let mut parent = storage.root_id();
        for i in 0..4 {
            let id = Id::new([10 + i; 32]);
            storage.add_entity_with_parent(id, parent, b"node", Metadata::default());
            parent = id;
        }

        assert_eq!(
            storage.max_depth(),
            5,
            "Chain of 5 nodes should have depth 5"
        );
    }

    #[test]
    fn test_max_depth_unbalanced() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());
        storage.init_root();

        // Create unbalanced tree:
        //       root
        //      /    \
        //     a      b
        //    /
        //   c
        //  /
        // d
        let a = Id::new([10u8; 32]);
        let b = Id::new([20u8; 32]);
        let c = Id::new([30u8; 32]);
        let d = Id::new([40u8; 32]);

        storage.add_entity_with_parent(a, storage.root_id(), b"a", Metadata::default());
        storage.add_entity_with_parent(b, storage.root_id(), b"b", Metadata::default());
        storage.add_entity_with_parent(c, a, b"c", Metadata::default());
        storage.add_entity_with_parent(d, c, b"d", Metadata::default());

        // Depth should be longest path: root -> a -> c -> d = 4
        assert_eq!(storage.max_depth(), 4, "Unbalanced tree depth should be 4");
    }

    #[test]
    fn test_delete_entity() {
        let storage = SimStorage::new(test_context_id(), test_executor_id());

        let id = Id::new([10u8; 32]);
        storage.add_entity(id, b"hello", Metadata::default());
        assert!(storage.has_entity(id));
        assert!(!storage.is_deleted(id));

        storage.remove_entity(id);
        // Entity index still exists (tombstone) but is marked deleted
        assert!(storage.is_deleted(id));
    }
}
