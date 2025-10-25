//! Storage interface implementing a repository pattern for CRDT-based data.
//!
//! This module provides the primary API for interacting with the storage system,
//! handling entity persistence, hierarchy management, and distributed synchronization.
//!
//! # Architecture
//!
//! Calimero uses a **hybrid CRDT model**:
//! - **Operation-based (CmRDT)**: Local changes emit [`Action`]s propagated to peers
//! - **State-based (CvRDT)**: Merkle tree comparison for catch-up/reconciliation
//!
//! Each element maintains two Merkle hashes (own data, and full including descendants)
//! enabling efficient tree comparison—only subtrees with differing hashes need examination.
//!
//! # API Entry Points
//!
//! **Direct Operations:**
//! - [`save()`](Interface::save()) - Save/update entities
//! - [`add_child_to()`](Interface::add_child_to()) - Add to collections
//! - [`remove_child_from()`](Interface::remove_child_from()) - Remove from collections
//! - [`find_by_id()`](Interface::find_by_id()) - Direct lookup
//!
//! **Synchronization:**
//! - [`apply_action()`](Interface::apply_action()) - Execute remote changes
//! - [`compare_trees()`](Interface::compare_trees()) - Generate sync actions
//!
//! # Conflict Resolution
//!
//! - Last-write-wins based on timestamps
//! - Orphaned children (from out-of-order ops) stored temporarily
//! - Future comparison reconciles inconsistencies
//!
//! See the [crate README](../README.md) for detailed design documentation.

#[cfg(test)]
#[path = "tests/interface.rs"]
mod tests;

use core::fmt::Debug;
use core::marker::PhantomData;
use std::collections::BTreeMap;

use borsh::{from_slice, to_vec, BorshDeserialize, BorshSerialize};
use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::address::{Id, Path};
use crate::entities::{ChildInfo, Collection, Data, Metadata};
use crate::env::time_now;
use crate::index::Index;
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::sync;

// Re-export for backward compatibility
pub use crate::error::StorageError;

/// Convenient type alias for the main storage system.
pub type MainInterface = Interface<MainStorage>;

/// Actions to be taken during synchronisation.
///
/// The following variants represent the possible actions arising from either a
/// direct change or a comparison between two nodes.
///
///   - **Direct change**: When a direct change is made, in other words, when
///     there is local activity that results in data modification to propagate
///     to other nodes, the possible resulting actions are [`Add`](Action::Add),
///     [`Delete`](Action::Delete), and [`Update`](Action::Update). A comparison
///     is not needed in this case, as the deltas are known, and assuming all of
///     the actions are carried out, the nodes will be in sync.
///
///   - **Comparison**: When a comparison is made between two nodes, the
///     possible resulting actions are [`Add`](Action::Add), [`Delete`](Action::Delete),
///     [`Update`](Action::Update), and [`Compare`](Action::Compare). The extra
///     comparison action arises in the case of tree traversal, where a child
///     entity is found to differ between the two nodes. In this case, the child
///     entity is compared, and the resulting actions are added to the list of
///     actions to be taken. This process is recursive.
///
/// Note: Some actions contain the full entity, and not just the entity ID, as
/// the actions will often be in context of data that is not available locally
/// and cannot be sourced otherwise. The actions are stored in serialised form
/// because of type restrictions, and they are due to be sent around the network
/// anyway.
///
/// Note: This enum contains the entity type, for passing to the guest for
/// processing along with the ID and data.
///
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[expect(clippy::exhaustive_enums, reason = "Exhaustive")]
pub enum Action {
    /// Add an entity with the given ID, type, and data.
    Add {
        /// Unique identifier of the entity.
        id: Id,

        /// Serialised data of the entity.
        data: Vec<u8>,

        /// Details of the ancestors of the entity.
        ancestors: Vec<ChildInfo>,

        /// The metadata of the entity.
        metadata: Metadata,
    },

    /// Compare the entity with the given ID and type. Note that this results in
    /// a direct comparison of the specific entity in question, including data
    /// that is immediately available to it, such as the hashes of its children.
    /// This may well result in further actions being generated if children
    /// differ, leading to a recursive comparison.
    Compare {
        /// Unique identifier of the entity.
        id: Id,
    },

    /// Delete an entity with the given ID.
    ///
    /// TODO: Deprecated in favor of DeleteRef. This variant carries full ancestor
    /// data which is inefficient. Kept for backward compatibility during migration.
    Delete {
        /// Unique identifier of the entity.
        id: Id,

        /// Details of the ancestors of the entity.
        ancestors: Vec<ChildInfo>,
    },

    /// Delete reference (tombstone-based deletion).
    ///
    /// More efficient than Delete variant - only sends ID and timestamp.
    /// Uses tombstone mechanism for proper CRDT semantics:
    /// - Handles delete vs update conflicts via timestamp comparison
    /// - Supports out-of-order message delivery
    /// - Enables 1-day retention + full resync strategy
    DeleteRef {
        /// Unique identifier of the entity to delete.
        id: Id,

        /// Timestamp when deletion occurred (for conflict resolution).
        deleted_at: u64,
    },

    /// Update the entity with the given ID and type to have the supplied data.
    Update {
        /// Unique identifier of the entity.
        id: Id,

        /// Serialised data of the entity.
        data: Vec<u8>,

        /// Details of the ancestors of the entity.
        ancestors: Vec<ChildInfo>,

        /// The metadata of the entity.
        metadata: Metadata,
    },
}

/// Data that is used for comparison between two nodes.
///
/// Uses two-level hashing to optimize sync:
/// - `full_hash` - Detects if **anything** in subtree changed
/// - `own_hash` - Detects if **this entity's data** changed (vs. children)
///
/// This enables skipping unchanged entity data during sync, only transferring
/// full data when the entity itself changed, not just its descendants.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ComparisonData {
    /// The unique identifier of the entity being compared.
    id: Id,

    /// Hash of entity's immediate data only (excludes children).
    /// Used to determine if entity itself changed vs. just its children.
    own_hash: [u8; 32],

    /// Hash of entity + all descendants.
    /// Used to quickly detect if any changes exist in subtree.
    full_hash: [u8; 32],

    /// The list of ancestors of the entity, with their IDs and hashes. The
    /// order is from the immediate parent to the root, so index zero will be
    /// the parent, and the last index will be the root.
    ancestors: Vec<ChildInfo>,

    /// The list of children of the entity, with their IDs and hashes,
    /// organised by collection name.
    children: BTreeMap<String, Vec<ChildInfo>>,

    /// The metadata of the entity.
    metadata: Metadata,
}

/// Snapshot of complete storage state for full resync.
///
/// Contains all entities and indexes, root hash for verification,
/// and metadata about the snapshot itself.
///
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize)]
pub struct Snapshot {
    /// All entity data (Key::Entry).
    pub entries: Vec<(Id, Vec<u8>)>,

    /// All index data (Key::Index).
    pub indexes: Vec<(Id, Vec<u8>)>,

    /// Root Merkle hash for verification.
    pub root_hash: [u8; 32],

    /// When the snapshot was created (nanoseconds since epoch).
    pub timestamp: u64,

    /// Number of entities in snapshot.
    pub entity_count: usize,

    /// Number of indexes in snapshot.
    pub index_count: usize,
}

/// Tracks synchronization state with a remote node.
///
/// Used to determine when full resync is needed vs incremental sync.
/// Persisted per remote node ID in storage under Key::SyncState.
///
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize, Eq, Ord, PartialEq, PartialOrd)]
pub struct SyncState {
    /// ID of the remote node this sync state tracks.
    pub node_id: Id,

    /// Timestamp of last successful sync (nanoseconds since epoch).
    pub last_sync_time: u64,

    /// Root hash at last sync (for validation).
    pub last_sync_root_hash: [u8; 32],

    /// Number of successful syncs with this node.
    pub sync_count: u64,
}

impl SyncState {
    /// Creates a new sync state for a node.
    pub fn new(node_id: Id) -> Self {
        Self {
            node_id,
            last_sync_time: time_now(),
            last_sync_root_hash: [0; 32],
            sync_count: 0,
        }
    }

    /// Checks if full resync is needed based on offline duration.
    ///
    /// Returns true if node has been offline longer than tombstone retention.
    pub fn needs_full_resync(&self, tombstone_retention_nanos: u64) -> bool {
        let offline_duration = time_now().saturating_sub(self.last_sync_time);
        offline_duration > tombstone_retention_nanos
    }

    /// Updates sync state after successful sync.
    pub fn update(&mut self, root_hash: [u8; 32]) {
        self.last_sync_time = time_now();
        self.last_sync_root_hash = root_hash;
        self.sync_count += 1;
    }
}

/// The primary interface for the storage system.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct Interface<S: StorageAdaptor = MainStorage>(PhantomData<S>);

impl<S: StorageAdaptor> Interface<S> {
    /// Adds a child entity to a parent's collection.
    ///
    /// Updates Merkle hashes and generates sync actions automatically.
    ///
    /// # Errors
    /// - `SerializationError` if child can't be encoded
    /// - `IndexNotFound` if parent doesn't exist
    ///
    pub fn add_child_to<C: Collection, D: Data>(
        parent_id: Id,
        collection: &C,
        child: &mut D,
    ) -> Result<bool, StorageError> {
        if !child.element().is_dirty() {
            return Ok(false);
        }

        let data = to_vec(child).map_err(|e| StorageError::SerializationError(e.into()))?;

        let own_hash = Sha256::digest(&data).into();

        <Index<S>>::add_child_to(
            parent_id,
            collection.name(),
            ChildInfo::new(child.id(), own_hash, child.element().metadata),
        )?;

        let Some(hash) = Self::save_raw(child.id(), data, child.element().metadata)? else {
            return Ok(false);
        };

        child.element_mut().is_dirty = false;
        child.element_mut().merkle_hash = hash;

        Ok(true)
    }

    /// Applies a synchronization action from a remote node.
    ///
    /// Handles Add/Update/Delete actions, creating missing ancestors if needed.
    /// Generates Compare action for hash verification after applying changes.
    ///
    /// # Errors
    /// - `DeserializationError` if action data is invalid
    /// - `ActionNotAllowed` if Compare action is passed directly
    ///
    pub fn apply_action(action: Action) -> Result<(), StorageError> {
        match action {
            Action::Add {
                id,
                data,
                // todo! we only need parent_id
                ancestors,
                metadata,
            }
            | Action::Update {
                id,
                data,
                ancestors,
                metadata,
            } => {
                let mut parent = None;
                for this in ancestors.iter().rev() {
                    let parent = parent.replace(this);

                    if <Index<S>>::has_index(this.id()) {
                        continue;
                    }

                    let Some(parent) = parent else {
                        <Index<S>>::add_root(*this)?;

                        continue;
                    };

                    <Index<S>>::add_child_to(
                        parent.id(),
                        "no collection, remove this nonsense",
                        *this,
                    )?;
                }

                if let Some(parent) = parent {
                    let own_hash = Sha256::digest(&data).into();

                    <Index<S>>::add_child_to(
                        parent.id(),
                        "no collection, remove this nonsense",
                        ChildInfo::new(id, own_hash, metadata),
                    )?;
                }

                if Self::save_internal(id, &data, metadata)?.is_none() {
                    // we didn't save anything, so we skip updating the ancestors
                    return Ok(());
                }

                sync::push_action(Action::Compare { id });
            }
            Action::Compare { .. } => {
                return Err(StorageError::ActionNotAllowed("Compare".to_owned()))
            }
            Action::Delete {
                id, ancestors: _, ..
            } => {
                // TODO: Legacy Delete action - needs parent_id and collection to properly call remove_child_from
                // For now, just delete the entry and mark index as deleted
                // This is incomplete - parent's children list won't be updated!
                // Migrate to DeleteRef variant for proper handling
                let _ignored = S::storage_remove(Key::Entry(id));
                let _ignored = <Index<S>>::mark_deleted(id, crate::env::time_now());
            }
            Action::DeleteRef { id, deleted_at } => {
                // Tombstone-based deletion with CRDT semantics
                if <Index<S>>::is_deleted(id)? {
                    // Already deleted - use later deletion timestamp
                    let _ignored = <Index<S>>::mark_deleted(id, deleted_at);
                } else if let Some(metadata) = <Index<S>>::get_metadata(id)? {
                    // Entity exists and not deleted - check if deletion is newer than last update
                    if deleted_at >= *metadata.updated_at {
                        // Deletion wins - delete data and mark index
                        let _ignored = S::storage_remove(Key::Entry(id));
                        let _ignored = <Index<S>>::mark_deleted(id, deleted_at);
                    }
                    // else: local update is newer, ignore deletion
                } else {
                    // Never seen this entity - create tombstone to prevent resurrection
                    // This handles out-of-order messages (delete arrives before create)
                    // TODO: Need to create a tombstone index entry here
                    // For now, just ignore (deletion of non-existent entity)
                }
            }
        };

        Ok(())
    }

    /// Retrieves all children in a collection.
    ///
    /// Returns deserialized child entities. Order is not guaranteed.
    ///
    /// # Errors
    /// - `IndexNotFound` if parent doesn't exist
    /// - `DeserializationError` if child data is corrupt
    ///
    pub fn children_of<C: Collection>(
        parent_id: Id,
        collection: &C,
    ) -> Result<Vec<C::Child>, StorageError> {
        let children_info = <Index<S>>::get_children_of(parent_id, collection.name())?;
        let mut children = Vec::new();
        for child_info in children_info {
            if let Some(child) = Self::find_by_id(child_info.id())? {
                children.push(child);
            }
        }
        Ok(children)
    }

    /// Retrieves child metadata without deserializing full data.
    ///
    /// Returns IDs, hashes, and timestamps only. More efficient than [`children_of()`](Self::children_of()).
    ///
    /// # Errors
    /// Returns error if index lookup fails.
    ///
    pub fn child_info_for<C: Collection>(
        parent_id: Id,
        collection: &C,
    ) -> Result<Vec<ChildInfo>, StorageError> {
        <Index<S>>::get_children_of(parent_id, collection.name())
    }

    /// Compares local and remote entity trees, generating sync actions.
    ///
    /// Compares Merkle hashes recursively, producing action lists for both sides.
    /// Returns `(local_actions, remote_actions)` to bring trees into sync.
    ///
    /// # Errors
    /// Returns error if index lookup or hash comparison fails.
    ///
    pub fn compare_trees(
        foreign_entity_data: Option<Vec<u8>>,
        foreign_index_data: ComparisonData,
    ) -> Result<(Vec<Action>, Vec<Action>), StorageError> {
        let mut actions = (vec![], vec![]);

        let id = foreign_index_data.id;

        let local_metadata = <Index<S>>::get_metadata(id)?;

        let Some(local_entity) = Self::find_by_id_raw(id) else {
            if let Some(foreign_entity) = foreign_entity_data {
                // Local entity doesn't exist, so we need to add it
                actions.0.push(Action::Add {
                    id,
                    data: foreign_entity,
                    ancestors: foreign_index_data.ancestors,
                    metadata: foreign_index_data.metadata,
                });
            }

            return Ok(actions);
        };

        let local_metadata = local_metadata.ok_or(StorageError::IndexNotFound(id))?;

        let (local_full_hash, local_own_hash) =
            <Index<S>>::get_hashes_for(id)?.ok_or(StorageError::IndexNotFound(id))?;

        // Compare full Merkle hashes
        if local_full_hash == foreign_index_data.full_hash {
            return Ok(actions);
        }

        // Compare own hashes and timestamps
        if local_own_hash != foreign_index_data.own_hash {
            match foreign_entity_data {
                Some(foreign_entity_data)
                    if local_metadata.updated_at <= foreign_index_data.metadata.updated_at =>
                {
                    actions.0.push(Action::Update {
                        id,
                        data: foreign_entity_data,
                        ancestors: foreign_index_data.ancestors,
                        metadata: foreign_index_data.metadata,
                    });
                }
                _ => {
                    actions.1.push(Action::Update {
                        id,
                        data: local_entity,
                        ancestors: <Index<S>>::get_ancestors_of(id)?,
                        metadata: local_metadata,
                    });
                }
            }
        }

        // The list of collections from the type will be the same on both sides, as
        // the type is the same.

        let local_collection_names = <Index<S>>::get_collection_names_for(id)?;

        let local_collections = local_collection_names
            .into_iter()
            .map(|name| {
                let children = <Index<S>>::get_children_of(id, &name)?;
                Ok((name, children))
            })
            .collect::<Result<BTreeMap<_, _>, StorageError>>()?;

        // Compare children
        for (local_coll_name, local_children) in &local_collections {
            if let Some(foreign_children) = foreign_index_data.children.get(local_coll_name) {
                let local_child_map: IndexMap<_, _> = local_children
                    .iter()
                    .map(|child| (child.id(), child.merkle_hash()))
                    .collect();
                let foreign_child_map: IndexMap<_, _> = foreign_children
                    .iter()
                    .map(|child| (child.id(), child.merkle_hash()))
                    .collect();

                for (child_id, local_hash) in &local_child_map {
                    match foreign_child_map.get(child_id) {
                        Some(foreign_hash) if local_hash != foreign_hash => {
                            actions.0.push(Action::Compare { id: *child_id });
                            actions.1.push(Action::Compare { id: *child_id });
                        }
                        None => {
                            if let Some(local_child) = Self::find_by_id_raw(*child_id) {
                                let metadata = <Index<S>>::get_metadata(*child_id)?
                                    .ok_or(StorageError::IndexNotFound(*child_id))?;

                                actions.1.push(Action::Add {
                                    id: *child_id,
                                    data: local_child,
                                    ancestors: <Index<S>>::get_ancestors_of(id)?,
                                    metadata,
                                });
                            }
                        }
                        // Hashes match, no action needed
                        _ => {}
                    }
                }

                for id in foreign_child_map.keys() {
                    if !local_child_map.contains_key(id) {
                        // Child exists in foreign but not locally, compare.
                        // We can't get the full data for the foreign child, so we flag it for
                        // comparison.
                        actions.1.push(Action::Compare { id: *id });
                    }
                }
            } else {
                // The entire collection is missing from the foreign entity
                for child in local_children {
                    if let Some(local_child) = Self::find_by_id_raw(child.id()) {
                        let metadata = <Index<S>>::get_metadata(child.id())?
                            .ok_or(StorageError::IndexNotFound(child.id()))?;

                        actions.1.push(Action::Add {
                            id: child.id(),
                            data: local_child,
                            ancestors: <Index<S>>::get_ancestors_of(child.id())?,
                            metadata,
                        });
                    }
                }
            }
        }

        // Check for collections in the foreign entity that don't exist locally
        for (foreign_coll_name, foreign_children) in &foreign_index_data.children {
            if !local_collections.contains_key(foreign_coll_name) {
                for child in foreign_children {
                    // We can't get the full data for the foreign child, so we flag it for comparison
                    actions.1.push(Action::Compare { id: child.id() });
                }
            }
        }

        Ok(actions)
    }

    /// Compares entities and automatically applies sync actions locally.
    ///
    /// Convenience wrapper around [`compare_trees()`](Self::compare_trees()) that applies
    /// local actions immediately and pushes remote actions to sync queue.
    ///
    /// # Errors
    /// Returns error if comparison or action application fails.
    ///
    pub fn compare_affective(
        data: Option<Vec<u8>>,
        comparison_data: ComparisonData,
    ) -> Result<(), StorageError> {
        let (local, remote) = <Interface<S>>::compare_trees(data, comparison_data)?;

        for action in local {
            if let Action::Compare { .. } = &action {
                continue;
            }

            <Interface<S>>::apply_action(action)?;
        }

        for action in remote {
            sync::push_action(action);
        }

        Ok(())
    }

    /// Finds and deserializes an entity by its unique ID.
    ///
    /// Filters out tombstoned (deleted) entities automatically.
    ///
    /// # Errors
    /// - `DeserializationError` if stored data is corrupt
    /// - `IndexNotFound` if entity exists but has no index
    ///
    pub fn find_by_id<D: Data>(id: Id) -> Result<Option<D>, StorageError> {
        // Check if entity is deleted (tombstone)
        if <Index<S>>::is_deleted(id)? {
            return Ok(None); // Entity is deleted
        }

        let value = S::storage_read(Key::Entry(id));

        let Some(slice) = value else {
            return Ok(None);
        };

        let mut item = from_slice::<D>(&slice).map_err(StorageError::DeserializationError)?;

        let (full_hash, _) =
            <Index<S>>::get_hashes_for(id)?.ok_or(StorageError::IndexNotFound(id))?;

        item.element_mut().merkle_hash = full_hash;

        item.element_mut().metadata =
            <Index<S>>::get_metadata(id)?.ok_or(StorageError::IndexNotFound(id))?;

        Ok(Some(item))
    }

    /// Finds an entity by ID, returning raw bytes without deserialization.
    ///
    /// Note: This does NOT filter deleted entities. Use `find_by_id` for automatic
    /// tombstone filtering.
    ///
    pub fn find_by_id_raw(id: Id) -> Option<Vec<u8>> {
        S::storage_read(Key::Entry(id))
    }

    /// Finds entities by hierarchical path.
    ///
    /// **Note**: Not yet implemented.
    ///
    /// # Errors
    /// Currently panics (unimplemented).
    ///
    pub fn find_by_path<D: Data>(_path: &Path) -> Result<Vec<D>, StorageError> {
        unimplemented!()
    }

    /// Finds children by parent ID and collection name.
    ///
    /// # Errors
    /// - `IndexNotFound` if parent doesn't exist
    /// - `DeserializationError` if child data is corrupt
    ///
    pub fn find_children_by_id<D: Data>(
        parent_id: Id,
        collection: &str,
    ) -> Result<Vec<D>, StorageError> {
        let child_infos = <Index<S>>::get_children_of(parent_id, collection)?;
        let mut children = Vec::new();
        for child_info in child_infos {
            if let Some(child) = Self::find_by_id(child_info.id())? {
                children.push(child);
            }
        }
        Ok(children)
    }

    /// Generates comparison metadata for tree synchronization.
    ///
    /// Includes hashes, ancestors, children info. Used by [`compare_trees()`](Self::compare_trees()).
    ///
    /// # Errors
    /// Returns error if index lookup fails.
    ///
    pub fn generate_comparison_data(id: Option<Id>) -> Result<ComparisonData, StorageError> {
        let id = id.unwrap_or_else(Id::root);

        let (full_hash, own_hash) = <Index<S>>::get_hashes_for(id)?.unwrap_or_default();

        let metadata = <Index<S>>::get_metadata(id)?.unwrap_or_default();

        let ancestors = <Index<S>>::get_ancestors_of(id)?;

        let collection_names = <Index<S>>::get_collection_names_for(id)?;

        let children = collection_names
            .into_iter()
            .map(|collection_name| {
                <Index<S>>::get_children_of(id, &collection_name)
                    .map(|children| (collection_name.clone(), children))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        Ok(ComparisonData {
            id,
            own_hash,
            full_hash,
            ancestors,
            children,
            metadata,
        })
    }

    /// Checks if a collection has any children.
    ///
    /// # Errors
    /// Returns error if index lookup fails.
    ///
    pub fn has_children<C: Collection>(
        parent_id: Id,
        collection: &C,
    ) -> Result<bool, StorageError> {
        <Index<S>>::has_children(parent_id, collection.name())
    }

    /// Retrieves the parent entity of a child.
    ///
    /// # Errors
    /// Returns error if index lookup or deserialization fails.
    ///
    pub fn parent_of<D: Data>(child_id: Id) -> Result<Option<D>, StorageError> {
        <Index<S>>::get_parent_id(child_id)?
            .map_or_else(|| Ok(None), |parent_id| Self::find_by_id(parent_id))
    }

    /// Removes a child from a collection.
    ///
    /// Deletes the child entity and generates sync actions automatically.
    ///
    /// # Errors
    /// Returns error if parent or child doesn't exist.
    ///
    pub fn remove_child_from<C: Collection>(
        parent_id: Id,
        collection: &C,
        child_id: Id,
    ) -> Result<bool, StorageError> {
        let child_exists = <Index<S>>::get_children_of(parent_id, collection.name())?
            .iter()
            .any(|child| child.id() == child_id);
        if !child_exists {
            return Ok(false);
        }

        <Index<S>>::remove_child_from(parent_id, collection.name(), child_id)?;

        let (parent_full_hash, _) =
            <Index<S>>::get_hashes_for(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;
        let mut ancestors = <Index<S>>::get_ancestors_of(parent_id)?;
        let metadata =
            <Index<S>>::get_metadata(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;
        ancestors.insert(0, ChildInfo::new(parent_id, parent_full_hash, metadata));

        _ = S::storage_remove(Key::Entry(child_id));

        sync::push_action(Action::Delete {
            id: child_id,
            ancestors,
        });

        Ok(true)
    }

    /// Retrieves the root entity.
    ///
    /// # Errors
    /// Returns error if deserialization fails.
    ///
    pub fn root<D: Data>() -> Result<Option<D>, StorageError> {
        Self::find_by_id(Id::root())
    }

    /// Saves the root entity and commits sync actions.
    ///
    /// Should be called at the end of each transaction. Call once per execution.
    ///
    /// # Errors
    /// - `UnexpectedId` if root ID doesn't match
    /// - `SerializationError` if encoding fails
    ///
    pub fn commit_root<D: Data>(root: Option<D>) -> Result<(), StorageError> {
        let id: Id = Id::root();

        let hash = if let Some(root) = root {
            if root.id() != id {
                return Err(StorageError::UnexpectedId(root.id()));
            }

            if !root.element().is_dirty() {
                return Ok(());
            }

            let data = to_vec(&root).map_err(|e| StorageError::SerializationError(e.into()))?;

            Self::save_raw(id, data, root.element().metadata)?
        } else {
            <Index<S>>::get_hashes_for(id)?.map(|(full_hash, _)| full_hash)
        };

        if let Some(hash) = hash {
            sync::commit_root(&hash)?;
        }

        Ok(())
    }

    /// Saves an entity to storage, updating if it exists.
    ///
    /// Only saves if entity is dirty. Returns `false` if not saved due to:
    /// - Entity not dirty
    /// - Existing record is newer (last-write-wins guard)
    ///
    /// Automatically:
    /// - Calculates Merkle hashes
    /// - Updates timestamps
    /// - Generates sync actions
    /// - Propagates hash changes up ancestor chain
    ///
    /// **Note**: Use [`add_child_to()`](Self::add_child_to()) for new children,
    /// then `save()` for subsequent updates.
    ///
    /// # Errors
    /// - `SerializationError` if encoding fails
    /// - `CannotCreateOrphan` if entity has no parent and isn't root
    ///
    pub fn save<D: Data>(entity: &mut D) -> Result<bool, StorageError> {
        if !entity.element().is_dirty() {
            return Ok(false);
        }

        let data = to_vec(entity).map_err(|e| StorageError::SerializationError(e.into()))?;

        let Some(hash) = Self::save_raw(entity.id(), data, entity.element().metadata)? else {
            return Ok(false);
        };

        entity.element_mut().is_dirty = false;
        entity.element_mut().merkle_hash = hash;

        Ok(true)
    }

    /// Saves raw data to the storage system.
    ///
    /// # Errors
    ///
    /// If an error occurs when serialising data or interacting with the storage
    /// system, an error will be returned.
    ///
    fn save_internal(
        id: Id,
        data: &[u8],
        metadata: Metadata,
    ) -> Result<Option<(bool, [u8; 32])>, StorageError> {
        let last_metadata = <Index<S>>::get_metadata(id)?;

        if let Some(last_metadata) = &last_metadata {
            if last_metadata.updated_at > metadata.updated_at {
                return Ok(None);
            }
        } else if id.is_root() {
            <Index<S>>::add_root(ChildInfo::new(id, [0_u8; 32], metadata))?;
        }

        let own_hash = Sha256::digest(data).into();

        let full_hash = <Index<S>>::update_hash_for(id, own_hash, Some(metadata.updated_at))?;

        _ = S::storage_write(Key::Entry(id), data);

        let is_new = metadata.created_at == *metadata.updated_at;

        Ok(Some((is_new, full_hash)))
    }

    /// Saves raw serialized data with orphan checking.
    ///
    /// # Errors
    /// - `CannotCreateOrphan` if entity has no parent and isn't root
    ///
    pub fn save_raw(
        id: Id,
        data: Vec<u8>,
        metadata: Metadata,
    ) -> Result<Option<[u8; 32]>, StorageError> {
        if !id.is_root() && <Index<S>>::get_parent_id(id)?.is_none() {
            return Err(StorageError::CannotCreateOrphan(id));
        }

        let Some((is_new, full_hash)) = Self::save_internal(id, &data, metadata)? else {
            return Ok(None);
        };

        let ancestors = <Index<S>>::get_ancestors_of(id)?;

        let action = if is_new {
            Action::Add {
                id,
                data,
                ancestors,
                metadata,
            }
        } else {
            Action::Update {
                id,
                data,
                ancestors,
                metadata,
            }
        };

        sync::push_action(action);

        Ok(Some(full_hash))
    }

    /// Validates Merkle tree integrity.
    ///
    /// **Note**: Not yet implemented.
    ///
    /// # Errors
    /// Currently panics (unimplemented).
    ///
    pub fn validate() -> Result<(), StorageError> {
        unimplemented!()
    }

    /// Sync orchestration with automatic incremental/full resync decision.
    ///
    /// Implements the hybrid sync strategy:
    /// - Incremental sync for normal case (< 1 day offline)
    /// - Full resync for extended offline (> 2 days)
    /// - Grace period with fallback (1-2 days offline)
    ///
    /// # Parameters
    ///
    /// * `remote_node_id` - ID of remote node to sync with
    /// * `fetch_snapshot_fn` - Callback to fetch snapshot from remote (network layer)
    ///
    /// # Strategy
    ///
    /// ```text
    /// if offline < TOMBSTONE_RETENTION (1 day):
    ///     → Incremental sync (compare_trees + apply_actions)
    /// else if offline < FULL_RESYNC_THRESHOLD (2 days):
    ///     → Try incremental, fallback to full resync on failure
    /// else:
    ///     → Full resync immediately
    /// ```
    ///
    /// # TODO
    ///
    /// - [ ] Integrate with network layer for automatic snapshot fetching
    /// - [ ] Add progress reporting callbacks
    /// - [ ] Handle split-brain (both nodes offline, need coordinator)
    /// - [ ] Add metrics (sync duration, bytes transferred, etc.)
    /// - [ ] Add sync coordination lock (prevent concurrent syncs)
    ///
    /// # Example Usage
    ///
    /// ```ignore
    /// use calimero_storage::constants::TOMBSTONE_RETENTION_NANOS;
    ///
    /// // Sync with remote node
    /// Interface::sync_with_node(
    ///     remote_node_id,
    ///     |node_id| {
    ///         // Network layer fetches snapshot
    ///         network::request_snapshot(node_id)
    ///     }
    /// )?;
    /// ```
    ///
    pub fn sync_with_node<F>(
        remote_node_id: Id,
        fetch_snapshot_fn: F,
    ) -> Result<(), StorageError>
    where
        F: FnOnce(Id) -> Result<Snapshot, StorageError>,
    {
        use crate::constants::{FULL_RESYNC_THRESHOLD_NANOS, TOMBSTONE_RETENTION_NANOS};

        // Get sync state
        let sync_state = Self::get_sync_state(remote_node_id)?;
        let offline_duration = sync_state
            .as_ref()
            .map(|s| time_now().saturating_sub(s.last_sync_time))
            .unwrap_or(0); // Never synced = 0 duration

        // Decision logic
        if offline_duration < TOMBSTONE_RETENTION_NANOS {
            // Normal case: Incremental sync
            // TODO: Implement incremental_sync() that uses compare_trees
            // For now, caller must use compare_trees + apply_action manually
            Ok(())
        } else if offline_duration < FULL_RESYNC_THRESHOLD_NANOS {
            // Grace period: Try incremental, fallback to full
            // TODO: Try incremental first
            // For now, go straight to full resync
            let snapshot = fetch_snapshot_fn(remote_node_id)?;
            Self::full_resync(remote_node_id, snapshot)
        } else {
            // Long offline: Full resync required
            let snapshot = fetch_snapshot_fn(remote_node_id)?;
            Self::full_resync(remote_node_id, snapshot)
        }
    }

    /// Gets the sync state for a remote node.
    ///
    /// Returns None if never synced with this node before.
    pub fn get_sync_state(node_id: Id) -> Result<Option<SyncState>, StorageError> {
        match S::storage_read(Key::SyncState(node_id)) {
            Some(data) => Ok(Some(
                SyncState::try_from_slice(&data).map_err(StorageError::DeserializationError)?,
            )),
            None => Ok(None),
        }
    }

    /// Saves sync state for a remote node.
    pub fn save_sync_state(state: &SyncState) -> Result<(), StorageError> {
        let data = to_vec(state).map_err(StorageError::SerializationError)?;
        let _ignored = S::storage_write(Key::SyncState(state.node_id), &data);
        Ok(())
    }

    /// Checks if full resync is needed with a remote node.
    ///
    /// # Parameters
    ///
    /// * `node_id` - Remote node to check
    /// * `tombstone_retention_nanos` - Tombstone retention period (e.g., 86_400_000_000_000 for 1 day)
    ///
    /// # Returns
    ///
    /// * `true` - Full resync needed (node offline > retention period)
    /// * `false` - Incremental sync OK
    ///
    pub fn needs_full_resync(
        node_id: Id,
        tombstone_retention_nanos: u64,
    ) -> Result<bool, StorageError> {
        match Self::get_sync_state(node_id)? {
            Some(state) => Ok(state.needs_full_resync(tombstone_retention_nanos)),
            None => Ok(false), // Never synced = not offline, can do incremental
        }
    }

    /// Full resync protocol.
    ///
    /// Completely rebuilds local state from remote node snapshot.
    /// Used when node has been offline longer than tombstone retention period.
    ///
    /// # Parameters
    ///
    /// * `remote_node_id` - ID of remote node to resync from
    /// * `snapshot` - Full snapshot data from remote node
    ///
    /// # Process
    ///
    /// 1. Validate snapshot integrity
    /// 2. Clear local storage (except SyncState)
    /// 3. Rebuild from snapshot
    /// 4. Verify Merkle root matches
    /// 5. Update sync state
    ///
    /// # Safety
    ///
    /// This function deletes all local data. Only call during controlled resync.
    ///
    /// # TODO
    ///
    /// - [ ] Add network protocol for fetching snapshot from remote
    /// - [ ] Handle "split brain" (both nodes want full resync - need coordinator)
    /// - [ ] Add streaming for large snapshots
    /// - [ ] Add progress callbacks
    /// - [ ] Add retry logic with exponential backoff
    /// - [ ] Add pre-resync backup for rollback
    ///
    /// # Example Usage
    ///
    /// ```ignore
    /// // Check if full resync is needed
    /// if Interface::needs_full_resync(remote_id, TOMBSTONE_RETENTION_NANOS)? {
    ///     // Fetch snapshot from remote (network call - not yet implemented)
    ///     let snapshot = fetch_snapshot_from_remote(remote_id)?;
    ///     
    ///     // Apply it locally
    ///     Interface::full_resync(remote_id, snapshot)?;
    /// }
    /// ```
    ///
    pub fn full_resync(remote_node_id: Id, snapshot: Snapshot) -> Result<(), StorageError> {
        // Step 1: Validate snapshot
        // TODO: Add more validation (age check, size limits, etc.)
        if snapshot.entity_count == 0 && snapshot.index_count == 0 {
            return Err(StorageError::InvalidData(
                "Snapshot is empty".to_owned(),
            ));
        }

        // Step 2: Apply snapshot (clears local storage)
        Self::apply_snapshot(&snapshot)?;

        // Step 3: Update sync state
        let mut sync_state = Self::get_sync_state(remote_node_id)?
            .unwrap_or_else(|| SyncState::new(remote_node_id));

        sync_state.update(snapshot.root_hash);
        Self::save_sync_state(&sync_state)?;

        Ok(())
    }

    /// Fetches a snapshot from a remote node.
    ///
    /// # TODO
    ///
    /// This is a placeholder for the network protocol implementation.
    /// Actual implementation needs:
    /// - Network layer integration
    /// - Snapshot request/response protocol
    /// - Chunking for large snapshots
    /// - Progress reporting
    /// - Timeout handling
    /// - Retry logic
    ///
    /// For now, this must be implemented by the caller using their
    /// network layer and passing the snapshot to full_resync().
    ///
    #[allow(dead_code, reason = "Placeholder for network implementation")]
    pub fn fetch_snapshot_from_remote(_remote_node_id: Id) -> Result<Snapshot, StorageError> {
        // TODO: Implement network protocol for snapshot transfer
        // This requires integration with the network layer which is
        // outside the scope of the storage module.
        //
        // Suggested approach:
        // 1. Send SNAPSHOT_REQUEST message to remote node
        // 2. Remote calls generate_snapshot() and sends SNAPSHOT_RESPONSE
        // 3. Receive and deserialize snapshot
        // 4. Handle chunking for large snapshots
        unimplemented!("Network protocol not yet implemented - implement in network layer")
    }

    /// Generates a full snapshot for resync.
    ///
    /// Exports all entities and indexes for transfer to a remote node.
    /// Excludes tombstones and SyncState (those are node-specific).
    ///
    /// # TODO
    ///
    /// - [ ] Add compression (gzip/zstd)
    /// - [ ] Support streaming for large datasets
    /// - [ ] Include schema version for compatibility checks
    /// - [ ] Add progress reporting callback
    ///
    pub fn generate_snapshot() -> Result<Snapshot, StorageError> {
        let mut entries = Vec::new();
        let mut indexes = Vec::new();

        // Iterate all storage keys
        let all_keys = S::storage_iter_keys();

        for key in all_keys {
            match key {
                Key::Entry(id) => {
                    // Only include entity data if not deleted
                    // If is_deleted returns an error, skip this entry to be safe
                    match <Index<S>>::is_deleted(id) {
                        Ok(false) => {
                            // Not deleted, include it
                            if let Some(data) = S::storage_read(key) {
                                entries.push((id, data));
                            }
                        }
                        Ok(true) => {
                            // Deleted (tombstone), skip
                        }
                        Err(_) => {
                            // No index found - orphaned entry, skip for safety
                        }
                    }
                }
                Key::Index(id) => {
                    // Only include non-deleted indexes
                    match <Index<S>>::is_deleted(id) {
                        Ok(false) => {
                            // Not a tombstone, include raw index data
                            if let Some(data) = S::storage_read(key) {
                                indexes.push((id, data));
                            }
                        }
                        Ok(true) => {
                            // Tombstone, skip
                        }
                        Err(_) => {
                            // Error reading index, skip
                        }
                    }
                }
                Key::SyncState(_) => {
                    // Skip sync state (node-specific metadata)
                }
            }
        }

        // Calculate root hash
        let root_hash = <Index<S>>::get_hashes_for(Id::root())?
            .map(|(full_hash, _)| full_hash)
            .unwrap_or([0; 32]);

        Ok(Snapshot {
            entity_count: entries.len(),
            index_count: indexes.len(),
            entries,
            indexes,
            root_hash,
            timestamp: time_now(),
        })
    }

    /// Applies a snapshot during full resync.
    ///
    /// Clears local storage and rebuilds from snapshot.
    /// Verifies Merkle root matches after rebuild.
    ///
    /// # Safety
    ///
    /// This function **deletes all local data** except SyncState.
    /// Only call during controlled resync operations.
    ///
    /// # TODO
    ///
    /// - [ ] Add validation before clearing storage
    /// - [ ] Support incremental application for large snapshots
    /// - [ ] Add rollback on errors
    /// - [ ] Add confirmation mechanism (prevent accidental wipes)
    ///
    pub fn apply_snapshot(snapshot: &Snapshot) -> Result<(), StorageError> {
        // Step 1: Clear all entity and index data
        // Preserve SyncState to track resync history
        Self::clear_all_storage_except_sync_state()?;

        // Step 2: Write all indexes first (needed for entry validation)
        for (id, index_data) in &snapshot.indexes {
            let _written = S::storage_write(Key::Index(*id), index_data);
        }

        // Step 3: Write all entities
        for (id, entry_data) in &snapshot.entries {
            let _written = S::storage_write(Key::Entry(*id), entry_data);
        }

        // Step 4: Verify root hash matches
        let local_root_hash = <Index<S>>::get_hashes_for(Id::root())?
            .map(|(full_hash, _)| full_hash)
            .unwrap_or([0; 32]);

        if local_root_hash != snapshot.root_hash {
            // TODO: Add rollback here - restore from backup or re-attempt
            return Err(StorageError::InvalidData(
                "Snapshot root hash mismatch after application".to_owned(),
            ));
        }

        Ok(())
    }

    /// Clears all storage except SyncState.
    ///
    /// Used during full resync to wipe local data while preserving sync history.
    ///
    /// # Safety
    ///
    /// This deletes all entity and index data. Only call during controlled resync.
    ///
    /// # TODO
    ///
    /// - [ ] Add confirmation mechanism
    /// - [ ] Add backup before clear (for rollback)
    /// - [ ] Add progress reporting for large clears
    ///
    fn clear_all_storage_except_sync_state() -> Result<(), StorageError> {
        let all_keys = S::storage_iter_keys();

        for key in all_keys {
            match key {
                Key::Entry(_) | Key::Index(_) => {
                    // Remove entity and index data
                    let _removed = S::storage_remove(key);
                }
                Key::SyncState(_) => {
                    // Preserve sync state to track resync history
                }
            }
        }

        Ok(())
    }
}
