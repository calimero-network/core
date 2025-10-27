//! Indexing system for efficient tree navigation.

#[cfg(test)]
#[path = "tests/index.rs"]
mod tests;

use core::marker::PhantomData;
use std::collections::{BTreeMap, BTreeSet};

use borsh::{to_vec, BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use crate::address::Id;
use crate::entities::{ChildInfo, Metadata, UpdatedAt};
use crate::env::time_now;
use crate::interface::StorageError;
use crate::store::{IterableStorage, Key, StorageAdaptor};

/// Index entry for an entity.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EntityIndex {
    /// Entity ID.
    id: Id,

    /// Parent ID.
    parent_id: Option<Id>,

    /// Children organized by collection name.
    children: BTreeMap<String, Vec<ChildInfo>>,

    /// Full hash (entity + descendants).
    full_hash: [u8; 32],

    /// Own hash (entity only).
    own_hash: [u8; 32],

    /// Entity metadata.
    pub metadata: Metadata,

    /// Tombstone marker. When set, entity data is deleted but index kept for CRDT sync.
    /// Enables proper conflict resolution (delete vs update) in distributed scenarios.
    /// Garbage collected after retention period (default: 1 day).
    pub deleted_at: Option<u64>,
}

/// Entity index manager.
#[derive(Debug)]
pub struct Index<S: StorageAdaptor>(PhantomData<S>);

impl<S: StorageAdaptor> Index<S> {
    /// Adds a child to a parent's collection.
    pub(crate) fn add_child_to(
        parent_id: Id,
        collection: &str,
        child: ChildInfo,
    ) -> Result<(), StorageError> {
        // Get or create parent index (might not exist after CRDT sync for nested collections)
        let mut parent_index = Self::get_index(parent_id)?.unwrap_or_else(|| EntityIndex {
            id: parent_id,
            parent_id: None,
            children: BTreeMap::new(),
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: Metadata::default(),
            deleted_at: None,
        });

        // Get or create child index
        let mut child_index = Self::get_index(child.id())?.unwrap_or_else(|| EntityIndex {
            id: child.id(),
            parent_id: None,
            children: BTreeMap::new(),
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: child.metadata,
            deleted_at: None,
        });
        child_index.parent_id = Some(parent_id);
        child_index.own_hash = child.merkle_hash();
        child_index.full_hash =
            Self::calculate_full_hash_from(child_index.own_hash, &child_index.children, false)?;
        Self::save_index(&child_index)?;

        let children = parent_index
            .children
            .entry(collection.to_owned())
            .or_insert_with(Vec::new);

        let mut ordered = children.drain(..).collect::<BTreeSet<_>>();

        let _ignored = ordered.replace(ChildInfo::new(
            child.id(),
            child_index.full_hash,
            child.metadata,
        ));

        children.extend(ordered.into_iter());

        parent_index.full_hash =
            Self::calculate_full_hash_from(parent_index.own_hash, &parent_index.children, false)?;
        Self::save_index(&parent_index)?;

        Self::recalculate_ancestor_hashes_for(parent_id)?;
        Ok(())
    }

    /// Adds a root entity (entity without a parent).
    ///
    /// # Errors
    /// Returns `StorageError` if index cannot be loaded or saved.
    pub fn add_root(root: ChildInfo) -> Result<(), StorageError> {
        let mut index = Self::get_index(root.id())?.unwrap_or_else(|| EntityIndex {
            id: root.id(),
            parent_id: None,
            children: BTreeMap::new(),
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: root.metadata,
            deleted_at: None,
        });
        index.own_hash = root.merkle_hash();
        Self::save_index(&index)?;
        Ok(())
    }

    /// Calculates full Merkle hash from own hash and children.
    ///
    /// Combines entity's own hash with child hashes. More efficient than
    /// `calculate_full_merkle_hash_for` when own_hash is already in memory.
    fn calculate_full_hash_from(
        own_hash: [u8; 32],
        children: &BTreeMap<String, Vec<ChildInfo>>,
        recalculate: bool,
    ) -> Result<[u8; 32], StorageError> {
        let mut hasher = Sha256::new();
        hasher.update(own_hash);

        for children_list in children.values() {
            for child in children_list {
                let child_hash = if recalculate {
                    Self::calculate_full_merkle_hash_for(child.id(), true)?
                } else {
                    child.merkle_hash()
                };
                hasher.update(child_hash);
            }
        }

        Ok(hasher.finalize().into())
    }

    /// Calculates full Merkle hash by loading from storage.
    ///
    /// Reads own_hash from index. Use `calculate_full_hash_from` when own_hash
    /// is already in memory to avoid redundant DB reads.
    pub(crate) fn calculate_full_merkle_hash_for(
        id: Id,
        recalculate: bool,
    ) -> Result<[u8; 32], StorageError> {
        let index = Self::get_index(id)?.ok_or(StorageError::IndexNotFound(id))?;
        Self::calculate_full_hash_from(index.own_hash, &index.children, recalculate)
    }

    /// Returns ancestors from immediate parent to root.
    pub(crate) fn get_ancestors_of(id: Id) -> Result<Vec<ChildInfo>, StorageError> {
        let mut ancestors = Vec::new();
        let mut current_id = id;

        while let Some(parent_id) = Self::get_parent_id(current_id)? {
            let (parent_full_hash, _) =
                Self::get_hashes_for(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;
            let metadata =
                Self::get_metadata(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;
            ancestors.push(ChildInfo::new(parent_id, parent_full_hash, metadata));
            current_id = parent_id;
        }

        Ok(ancestors)
    }

    /// Returns entity metadata.
    pub(crate) fn get_metadata(id: Id) -> Result<Option<Metadata>, StorageError> {
        Ok(Self::get_index(id)?.map(|index| index.metadata))
    }

    /// Checks if an entity is deleted (tombstone marker set).
    ///
    /// Returns false if entity has no index (not found).
    ///
    /// # Errors
    /// Returns `StorageError` if index cannot be loaded or deserialized.
    pub fn is_deleted(id: Id) -> Result<bool, StorageError> {
        Ok(Self::get_index(id)?
            .and_then(|index| index.deleted_at)
            .is_some())
    }

    /// Marks an entity as deleted (sets tombstone).
    pub(crate) fn mark_deleted(id: Id, deleted_at: u64) -> Result<(), StorageError> {
        if let Some(mut index) = Self::get_index(id)? {
            index.deleted_at = Some(deleted_at);
            Self::save_index(&index)?;
        }
        Ok(())
    }

    /// Returns children from a specific collection.
    pub(crate) fn get_children_of(
        parent_id: Id,
        collection: &str,
    ) -> Result<Vec<ChildInfo>, StorageError> {
        Ok(Self::get_index(parent_id)?
            .ok_or(StorageError::IndexNotFound(parent_id))?
            .children
            .get(collection)
            .cloned()
            .unwrap_or_default())
    }

    /// Returns all collection names for an entity.
    pub(crate) fn get_collection_names_for(parent_id: Id) -> Result<Vec<String>, StorageError> {
        Ok(Self::get_index(parent_id)?
            .iter()
            .flat_map(|e| e.children.keys())
            .cloned()
            .collect())
    }

    /// Returns (full_hash, own_hash) tuple for an entity.
    ///
    /// # Errors
    /// Returns `StorageError` if index cannot be loaded or deserialized.
    #[expect(clippy::type_complexity, reason = "Not too complex")]
    pub fn get_hashes_for(id: Id) -> Result<Option<([u8; 32], [u8; 32])>, StorageError> {
        Ok(Self::get_index(id)?.map(|index| (index.full_hash, index.own_hash)))
    }

    /// Loads entity index from storage.
    pub(crate) fn get_index(id: Id) -> Result<Option<EntityIndex>, StorageError> {
        match S::storage_read(Key::Index(id)) {
            Some(data) => Ok(Some(
                EntityIndex::try_from_slice(&data).map_err(StorageError::DeserializationError)?,
            )),
            None => Ok(None),
        }
    }

    /// Checks if an entity has an index.
    pub(crate) fn has_index(id: Id) -> bool {
        S::storage_read(Key::Index(id)).is_some()
    }

    /// Returns the parent ID of an entity.
    pub(crate) fn get_parent_id(child_id: Id) -> Result<Option<Id>, StorageError> {
        Ok(Self::get_index(child_id)?.and_then(|index| index.parent_id))
    }

    /// Checks if a collection has any children.
    pub(crate) fn has_children(parent_id: Id, collection: &str) -> Result<bool, StorageError> {
        let parent_index =
            Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

        Ok(parent_index
            .children
            .get(collection)
            .map_or(false, |children| !children.is_empty()))
    }

    /// Recalculates ancestor hashes recursively up to root.
    pub(crate) fn recalculate_ancestor_hashes_for(id: Id) -> Result<(), StorageError> {
        let mut current_id = id;

        while let Some(parent_id) = Self::get_parent_id(current_id)? {
            let mut parent_index =
                Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

            // Update the child's hash in the parent's children list
            for children in &mut parent_index.children.values_mut() {
                if let Some(child) = children.iter_mut().find(|c| c.id() == current_id) {
                    let new_child_hash = Self::calculate_full_merkle_hash_for(current_id, false)?;
                    if child.merkle_hash() != new_child_hash {
                        *child = ChildInfo::new(current_id, new_child_hash, child.metadata);
                    }
                    break;
                }
            }

            // Recalculate the parent's full hash
            parent_index.full_hash = Self::calculate_full_hash_from(
                parent_index.own_hash,
                &parent_index.children,
                false,
            )?;
            Self::save_index(&parent_index)?;
            current_id = parent_id;
        }

        Ok(())
    }

    /// Removes and deletes a child from a collection.
    ///
    /// Uses tombstone-based deletion. To move a child to a different parent,
    /// just add it to the new parent instead.
    pub(crate) fn remove_child_from(
        parent_id: Id,
        collection: &str,
        child_id: Id,
    ) -> Result<(), StorageError> {
        Self::delete_entity_and_create_tombstone(child_id)?;
        Self::update_parent_after_child_removal(parent_id, collection, child_id)?;
        Self::recalculate_ancestor_hashes_for(parent_id)?;
        Ok(())
    }

    /// Deletes entity data and creates tombstone marker.
    ///
    /// Step 1 of deletion: Remove actual data, keep index for CRDT sync.
    fn delete_entity_and_create_tombstone(id: Id) -> Result<(), StorageError> {
        // Delete the actual entity data immediately (save storage space)
        let _ignored = S::storage_remove(Key::Entry(id));

        // Mark child index as deleted (tombstone for CRDT sync)
        Self::mark_deleted(id, time_now())
    }

    /// Updates parent's children list and hash after child removal.
    ///
    /// Step 2 of deletion: Remove child from parent's index and recalculate hash.
    fn update_parent_after_child_removal(
        parent_id: Id,
        collection: &str,
        child_id: Id,
    ) -> Result<(), StorageError> {
        let mut parent_index =
            Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

        // Remove child from collection
        if let Some(children) = parent_index.children.get_mut(collection) {
            children.retain(|child| child.id() != child_id);
        }

        // Recalculate parent's hash
        parent_index.full_hash =
            Self::calculate_full_hash_from(parent_index.own_hash, &parent_index.children, false)?;
        Self::save_index(&parent_index)?;

        Ok(())
    }

    /// Removes an entity's index from storage.
    #[cfg(test)]
    pub(crate) fn remove_index(id: Id) {
        _ = S::storage_remove(Key::Index(id));
    }

    /// Saves entity index to storage.
    pub(crate) fn save_index(index: &EntityIndex) -> Result<(), StorageError> {
        _ = S::storage_write(
            Key::Index(index.id),
            &to_vec(index).map_err(StorageError::SerializationError)?,
        );
        Ok(())
    }

    /// Updates entity's own_hash and recalculates full_hash.
    ///
    /// Returns the calculated full_hash (includes descendants).
    pub(crate) fn update_hash_for(
        id: Id,
        merkle_hash: [u8; 32],
        updated_at: Option<UpdatedAt>,
    ) -> Result<[u8; 32], StorageError> {
        let mut index = Self::get_index(id)?.ok_or(StorageError::IndexNotFound(id))?;
        index.own_hash = merkle_hash;
        index.full_hash = Self::calculate_full_hash_from(index.own_hash, &index.children, false)?;
        if let Some(updated_at) = updated_at {
            index.metadata.updated_at = updated_at;
        }
        Self::save_index(&index)?;
        <Index<S>>::recalculate_ancestor_hashes_for(id)?;
        Ok(index.full_hash)
    }

    /// Garbage collects tombstones older than the retention period.
    ///
    /// Only available for storage backends that implement `IterableStorage`.
    /// Removes index entries marked as deleted that are older than the specified
    /// retention period. This reclaims storage space while maintaining CRDT semantics
    /// for recent deletions.
    ///
    /// # Parameters
    ///
    /// * `retention_nanos` - Retention period in nanoseconds (e.g., 86_400_000_000_000 for 1 day)
    ///
    /// # Returns
    ///
    /// Number of tombstones garbage collected
    ///
    /// # Example
    ///
    /// ```ignore
    /// // GC tombstones older than 1 day (requires IterableStorage)
    /// type MyStorage = MockedStorage<1>;
    /// const ONE_DAY_NANOS: u64 = 86_400_000_000_000;
    /// let collected = Index::<MyStorage>::garbage_collect_tombstones(ONE_DAY_NANOS)?;
    /// println!("Garbage collected {} tombstones", collected);
    /// ```
    ///
    /// # Future Enhancements
    ///
    /// - Add metrics/logging for GC operations
    /// - Support batched deletion for large tombstone counts
    /// - [ ] Consider partial GC (limit number of items per run to avoid blocking)
    /// - [ ] Add GC scheduling mechanism (auto-run periodically)
    /// - [ ] Add GC configuration (min age, batch size, etc.)
    ///
    #[allow(dead_code, reason = "planned feature for tombstone cleanup")]
    pub(crate) fn garbage_collect_tombstones(retention_nanos: u64) -> Result<usize, StorageError>
    where
        S: IterableStorage,
    {
        let cutoff_time = time_now().saturating_sub(retention_nanos);
        let mut collected = 0;

        // Iterate over all keys in storage
        let all_keys = S::storage_iter_keys();

        for key in all_keys {
            // Only process Index keys (not Entry keys)
            if let Key::Index(id) = key {
                // Check if this index is a tombstone older than cutoff
                if let Some(index) = Self::get_index(id)? {
                    if let Some(deleted_at) = index.deleted_at {
                        if deleted_at < cutoff_time {
                            // Tombstone is old enough - remove it
                            let _ignored = S::storage_remove(Key::Index(id));
                            collected += 1;
                        }
                    }
                }
            }
        }

        Ok(collected)
    }
}
