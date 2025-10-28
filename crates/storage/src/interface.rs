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

use borsh::{from_slice, to_vec};
use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::address::{Id, Path};
use crate::entities::{ChildInfo, Collection, Data, Metadata};
use crate::env::time_now;
use crate::index::Index;
use crate::store::{Key, MainStorage, StorageAdaptor};

// Re-export types for convenience
pub use crate::action::{Action, ComparisonData};
pub use crate::error::StorageError;

/// Convenient type alias for the main storage system.
pub type MainInterface = Interface<MainStorage>;

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
                // Note: We track both parent and collection for full metadata,
                // though parent_id alone would suffice for tree structure
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

                // Pre-compute hash for adding to parent index (creates index if needed)
                if let Some(parent) = parent {
                    let own_hash = Sha256::digest(&data).into();
                    <Index<S>>::add_child_to(
                        parent.id(),
                        "no collection, remove this nonsense",
                        ChildInfo::new(id, own_hash, metadata),
                    )?;
                }

                // Save data (might merge, producing different hash)
                let Some((_, _full_hash)) = Self::save_internal(id, &data, metadata)? else {
                    // we didn't save anything, so we skip updating the ancestors
                    return Ok(());
                };

                // If data was merged, update parent's children list with correct hash
                if let Some(parent) = parent {
                    let (_, own_hash) =
                        <Index<S>>::get_hashes_for(id)?.ok_or(StorageError::IndexNotFound(id))?;

                    // Only update if hash changed due to merging
                    let parent_children = <Index<S>>::get_children_of(
                        parent.id(),
                        "no collection, remove this nonsense",
                    )?;
                    if let Some(child_info) = parent_children.iter().find(|c| c.id() == id) {
                        if child_info.merkle_hash() != own_hash {
                            // Hash changed due to merge - update parent
                            <Index<S>>::add_child_to(
                                parent.id(),
                                "no collection, remove this nonsense",
                                ChildInfo::new(id, own_hash, metadata),
                            )?;
                        }
                    }
                }

                crate::delta::push_action(Action::Compare { id });
            }
            Action::Compare { .. } => {
                return Err(StorageError::ActionNotAllowed("Compare".to_owned()))
            }
            Action::DeleteRef { id, deleted_at } => {
                Self::apply_delete_ref_action(id, deleted_at)?;
            }
        };

        Ok(())
    }

    /// Applies DeleteRef action with CRDT conflict resolution.
    ///
    /// Uses guard clauses for clarity (KISS principle).
    /// Handles three cases:
    /// 1. Already deleted - update tombstone if newer
    /// 2. Exists locally - compare timestamps (LWW)
    /// 3. Never seen - ignore (could create tombstone in future)
    fn apply_delete_ref_action(id: Id, deleted_at: u64) -> Result<(), StorageError> {
        // Guard: Already deleted, check if this deletion is newer
        if <Index<S>>::is_deleted(id)? {
            // Already has tombstone, use later deletion timestamp
            let _ignored = <Index<S>>::mark_deleted(id, deleted_at);
            return Ok(());
        }

        // Guard: Entity doesn't exist, nothing to delete
        let Some(metadata) = <Index<S>>::get_metadata(id)? else {
            // Entity doesn't exist - no tombstone needed
            // CRDT rationale: Deleting non-existent entity is idempotent no-op.
            // We don't create "preventive tombstones" because:
            // - Storage efficiency: Avoid bloat from phantom deletions
            // - CRDT convergence: If entity never existed, all peers agree (empty set)
            // - Idempotency: Safe to call remove_child_from multiple times
            return Ok(());
        };

        // Guard: Local update is newer, deletion loses
        if deleted_at < *metadata.updated_at {
            // Local update wins, ignore older deletion
            return Ok(());
        }

        // Deletion wins - apply it
        let _ignored = S::storage_remove(Key::Entry(id));
        let _ignored = <Index<S>>::mark_deleted(id, deleted_at);

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
            crate::delta::push_action(action);
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

    /// Gets raw entity data by ID.
    ///
    /// This is a simple alias for `find_by_id_raw` for convenience in tests.
    ///
    /// # Errors
    /// Returns `IndexNotFound` if entity doesn't exist.
    ///
    pub fn get(id: Id) -> Result<Vec<u8>, StorageError> {
        Self::find_by_id_raw(id).ok_or(StorageError::IndexNotFound(id))
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

        let deleted_at = time_now();

        <Index<S>>::remove_child_from(parent_id, collection.name(), child_id)?;

        // Use DeleteRef for efficient tombstone-based deletion
        // More efficient than Delete: only sends ID + timestamp vs full ancestor tree
        // The tombstone is created by remove_child_from, we just broadcast the deletion
        crate::delta::push_action(Action::DeleteRef {
            id: child_id,
            deleted_at,
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
            crate::delta::commit_root(&hash)?;
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

        let final_data = if let Some(last_metadata) = &last_metadata {
            if last_metadata.updated_at > metadata.updated_at {
                // Incoming is older - skip completely
                return Ok(None);
            } else if id.is_root() {
                // Root entity (app state) - ALWAYS merge to preserve CRDTs like G-Counter
                // Even if incoming is newer, we merge to avoid losing concurrent updates
                if let Some(existing_data) = S::storage_read(Key::Entry(id)) {
                    Self::try_merge_data(
                        id,
                        &existing_data,
                        data,
                        *last_metadata.updated_at,
                        *metadata.updated_at,
                    )?
                } else {
                    data.to_vec()
                }
            } else if last_metadata.updated_at == metadata.updated_at {
                // Concurrent update (same timestamp) - try to merge
                if let Some(existing_data) = S::storage_read(Key::Entry(id)) {
                    Self::try_merge_data(
                        id,
                        &existing_data,
                        data,
                        *last_metadata.updated_at,
                        *metadata.updated_at,
                    )?
                } else {
                    data.to_vec()
                }
            } else {
                // Incoming is newer - use it (LWW for non-root entities)
                data.to_vec()
            }
        } else {
            if id.is_root() {
                <Index<S>>::add_root(ChildInfo::new(id, [0_u8; 32], metadata))?;
            }
            data.to_vec()
        };

        let own_hash = Sha256::digest(&final_data).into();

        let full_hash = <Index<S>>::update_hash_for(id, own_hash, Some(metadata.updated_at))?;

        _ = S::storage_write(Key::Entry(id), &final_data);

        let is_new = metadata.created_at == *metadata.updated_at;

        Ok(Some((is_new, full_hash)))
    }

    /// Attempt to merge two versions of data using CRDT semantics.
    ///
    /// Returns the merged data, falling back to LWW (newer data) on failure.
    fn try_merge_data(
        _id: Id,
        existing: &[u8],
        incoming: &[u8],
        existing_timestamp: u64,
        incoming_timestamp: u64,
    ) -> Result<Vec<u8>, StorageError> {
        use crate::merge::merge_root_state;

        // Attempt CRDT merge
        match merge_root_state(existing, incoming, existing_timestamp, incoming_timestamp) {
            Ok(merged) => Ok(merged),
            Err(_) => {
                // Merge failed - fall back to LWW
                if incoming_timestamp >= existing_timestamp {
                    Ok(incoming.to_vec())
                } else {
                    Ok(existing.to_vec())
                }
            }
        }
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

        crate::delta::push_action(action);

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

    // NOTE: Sync orchestration moved to node layer (YAGNI)
    //
    // The sync decision logic should be in the node's sync manager, not in storage.
    // Storage provides primitives (generate_snapshot, full_resync, needs_full_resync),
    // but the orchestration belongs in the network/node layer where it can access
    // network protocols and handle peer communication.
    //
    // Example node layer implementation:
    //
    // ```rust
    // impl SyncManager {
    //     async fn sync_with_peer(&self, peer_id: NodeId) -> Result<()> {
    //         use calimero_storage::constants::TOMBSTONE_RETENTION_NANOS;
    //
    //         if Interface::needs_full_resync(peer_id, TOMBSTONE_RETENTION_NANOS)? {
    //             let snapshot = network::request_snapshot(peer_id).await?;
    //             Interface::full_resync(peer_id, snapshot)?;
    //         } else {
    //             // Incremental sync via compare_trees
    //             let comparison = network::request_comparison(peer_id).await?;
    //             let (local_actions, remote_actions) = Interface::compare_trees(comparison)?;
    //             // Apply actions...
    //         }
    //         Ok(())
    //     }
    // }
    // ```
}
