//! Indexing system for efficient tree navigation.

#[cfg(test)]
#[path = "tests/index.rs"]
mod tests;

use core::marker::PhantomData;
use std::collections::BTreeSet;

use borsh::{to_vec, BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use tracing::info;

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

    /// Children list.
    ///
    /// Collection name not stored - entity can only have one collection,
    /// so the name is redundant. API still accepts collection param for
    /// backwards compatibility but it's ignored internally.
    children: Option<Vec<ChildInfo>>,

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

impl EntityIndex {
    /// Returns the entity ID.
    #[must_use]
    pub fn id(&self) -> Id {
        self.id
    }

    /// Returns the parent ID, if any.
    #[must_use]
    pub fn parent_id(&self) -> Option<Id> {
        self.parent_id
    }

    /// Returns the children, if any.
    #[must_use]
    pub fn children(&self) -> Option<&[ChildInfo]> {
        self.children.as_deref()
    }

    /// Returns the full hash (entity + all descendants).
    #[must_use]
    pub fn full_hash(&self) -> [u8; 32] {
        self.full_hash
    }

    /// Returns the own hash (entity data only).
    #[must_use]
    pub fn own_hash(&self) -> [u8; 32] {
        self.own_hash
    }
}

/// Entity index manager.
#[derive(Debug)]
pub struct Index<S: StorageAdaptor>(PhantomData<S>);

impl<S: StorageAdaptor> Index<S> {
    /// Adds a child to a parent's collection.
    pub(crate) fn add_child_to(parent_id: Id, child: ChildInfo) -> Result<(), StorageError> {
        // Get or create parent index
        let mut parent_index = Self::get_index(parent_id)?.unwrap_or_else(|| EntityIndex {
            id: parent_id,
            parent_id: None,
            children: None,
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: Metadata::default(),
            deleted_at: None,
        });

        // Get or create child index
        let mut child_index = Self::get_index(child.id())?.unwrap_or_else(|| EntityIndex {
            id: child.id(),
            parent_id: None,
            children: None,
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: child.metadata.clone(),
            deleted_at: None,
        });
        child_index.parent_id = Some(parent_id);
        child_index.own_hash = child.merkle_hash();
        child_index.full_hash =
            Self::calculate_full_hash_for_children(child_index.own_hash, &child_index.children)?;
        Self::save_index(&child_index)?;

        // Get or create the children list
        // Collection name param is ignored - entity can only have one collection
        let children_vec = parent_index.children.get_or_insert_with(Vec::new);

        let mut ordered = children_vec.drain(..).collect::<BTreeSet<_>>();

        let _ignored = ordered.replace(ChildInfo::new(
            child.id(),
            child_index.full_hash,
            child.metadata,
        ));

        *children_vec = ordered.into_iter().collect();

        parent_index.full_hash =
            Self::calculate_full_hash_for_children(parent_index.own_hash, &parent_index.children)?;
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
            children: None,
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: root.metadata.clone(),
            deleted_at: None,
        });
        index.own_hash = root.merkle_hash();
        Self::save_index(&index)?;
        Ok(())
    }

    /// Calculates full Merkle hash from own hash and children.
    fn calculate_full_hash_for_children(
        own_hash: [u8; 32],
        children: &Option<Vec<ChildInfo>>,
    ) -> Result<[u8; 32], StorageError> {
        let mut hasher = Sha256::new();
        hasher.update(own_hash);

        if let Some(children_vec) = children {
            for child in children_vec {
                hasher.update(child.merkle_hash());
            }
        }

        Ok(hasher.finalize().into())
    }

    /// Calculates full Merkle hash by loading from storage.
    pub(crate) fn calculate_full_merkle_hash_for(id: Id) -> Result<[u8; 32], StorageError> {
        let index = Self::get_index(id)?.ok_or(StorageError::IndexNotFound(id))?;
        Self::calculate_full_hash_for_children(index.own_hash, &index.children)
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

            // Also update the `updated_at` timestamp to this nonce.
            // This is critical for replay protection on delete actions.
            *index.metadata.updated_at = deleted_at;

            Self::save_index(&index)?;
        }
        Ok(())
    }

    /// Returns children from a specific collection.
    ///
    /// Collection param is ignored - entity only has one collection.
    /// Kept in API for backwards compatibility.
    ///
    /// # Errors
    /// Returns `StorageError` if index cannot be loaded or deserialized.
    pub fn get_children_of(parent_id: Id) -> Result<Vec<ChildInfo>, StorageError> {
        let index = Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

        Ok(index.children.unwrap_or_default())
    }

    /// Returns all collection names for an entity.
    ///
    /// Legacy function - always returns at most one name (or empty).
    /// Kept for backwards compatibility with tree comparison logic.
    pub(crate) fn get_collection_names_for(parent_id: Id) -> Result<Vec<String>, StorageError> {
        // Return a dummy name if children exist, for tree comparison
        // The actual name doesn't matter since it's not stored
        Ok(Self::get_index(parent_id)?
            .and_then(|index| index.children.as_ref().map(|_| vec!["_".to_owned()]))
            .unwrap_or_default())
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
    ///
    /// Returns the full `EntityIndex` for an entity if it exists.
    /// Used by sync protocols to traverse the Merkle tree.
    ///
    /// # Errors
    /// Returns `StorageError` if index cannot be loaded or deserialized.
    pub fn get_index(id: Id) -> Result<Option<EntityIndex>, StorageError> {
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
    ///
    /// Collection param ignored - just checks if entity has any children.
    pub(crate) fn has_children(parent_id: Id) -> Result<bool, StorageError> {
        let parent_index =
            Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

        Ok(parent_index
            .children
            .as_ref()
            .map_or(false, |c| !c.is_empty()))
    }

    /// Recalculates ancestor hashes recursively up to root.
    pub(crate) fn recalculate_ancestor_hashes_for(id: Id) -> Result<(), StorageError> {
        let mut current_id = id;

        while let Some(parent_id) = Self::get_parent_id(current_id)? {
            let mut parent_index =
                Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;
            let old_full_hash = parent_index.full_hash;

            // Update the child's hash in the parent's children list
            if let Some(children) = &mut parent_index.children {
                if let Some(child) = children.iter_mut().find(|c| c.id() == current_id) {
                    let new_child_hash = Self::calculate_full_merkle_hash_for(current_id)?;
                    if child.merkle_hash() != new_child_hash {
                        // Log when a child's hash changes and affects the root
                        if parent_id.is_root() {
                            info!(
                                target: "storage::merkle",
                                child_id = %current_id,
                                old_child_hash = %hex::encode(child.merkle_hash()),
                                new_child_hash = %hex::encode(&new_child_hash),
                                "ROOT MERKLE: Child hash updated"
                            );
                        }
                        *child = ChildInfo::new(current_id, new_child_hash, child.metadata.clone());
                    }
                }
            }

            // Recalculate the parent's full hash
            parent_index.full_hash = Self::calculate_full_hash_for_children(
                parent_index.own_hash,
                &parent_index.children,
            )?;

            // Log when root hash changes
            if parent_id.is_root() && old_full_hash != parent_index.full_hash {
                let children_count = parent_index.children.as_ref().map(|c| c.len()).unwrap_or(0);
                info!(
                    target: "storage::merkle",
                    parent_id = %parent_id,
                    old_full_hash = %hex::encode(&old_full_hash),
                    new_full_hash = %hex::encode(&parent_index.full_hash),
                    children_count,
                    "ROOT MERKLE: Root hash recalculated from ancestor"
                );
            }

            Self::save_index(&parent_index)?;
            current_id = parent_id;
        }

        Ok(())
    }

    /// Removes and deletes a child from a collection.
    ///
    /// Uses tombstone-based deletion. To move a child to a different parent,
    /// just add it to the new parent instead.
    pub(crate) fn remove_child_from(parent_id: Id, child_id: Id) -> Result<(), StorageError> {
        Self::delete_entity_and_create_tombstone(child_id)?;
        Self::update_parent_after_child_removal(parent_id, child_id)?;
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

    /// Removes a child reference from a parent without creating a tombstone.
    ///
    /// Used when reassigning collection IDs - we need to remove the old child
    /// reference from the parent but don't want to create a tombstone since
    /// the collection is being moved, not deleted.
    pub(crate) fn remove_child_reference_only(
        parent_id: Id,
        child_id: Id,
    ) -> Result<(), StorageError> {
        Self::update_parent_after_child_removal(parent_id, child_id)?;
        Self::recalculate_ancestor_hashes_for(parent_id)?;
        Ok(())
    }

    /// Updates parent's children list and hash after child removal.
    ///
    /// Step 2 of deletion: Remove child from parent's index and recalculate hash.
    /// Made pub(crate) so Interface::apply_delete_ref_action can use it.
    pub(crate) fn update_parent_after_child_removal(
        parent_id: Id,
        child_id: Id,
    ) -> Result<(), StorageError> {
        let mut parent_index =
            Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

        // Remove child from collection (collection name ignored)
        if let Some(children) = &mut parent_index.children {
            children.retain(|child| child.id() != child_id);
            // Clear children if empty
            if children.is_empty() {
                parent_index.children = None;
            }
        }

        // Recalculate parent's hash
        parent_index.full_hash =
            Self::calculate_full_hash_for_children(parent_index.own_hash, &parent_index.children)?;
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
        let old_own_hash = index.own_hash;
        let old_full_hash = index.full_hash;
        index.own_hash = merkle_hash;
        index.full_hash = Self::calculate_full_hash_for_children(index.own_hash, &index.children)?;
        if let Some(updated_at) = updated_at {
            index.metadata.updated_at = updated_at;
        }

        // Log detailed info for root entity hash updates
        if id.is_root() {
            let children_count = index.children.as_ref().map(|c| c.len()).unwrap_or(0);
            let children_hashes: Vec<String> = index
                .children
                .as_ref()
                .map(|c| {
                    c.iter()
                        .map(|child| format!("{}:{}", child.id(), hex::encode(child.merkle_hash())))
                        .collect()
                })
                .unwrap_or_default();
            info!(
                target: "storage::merkle",
                %id,
                old_own_hash = %hex::encode(&old_own_hash),
                new_own_hash = %hex::encode(&merkle_hash),
                old_full_hash = %hex::encode(&old_full_hash),
                new_full_hash = %hex::encode(&index.full_hash),
                children_count,
                children_hashes = ?children_hashes,
                "ROOT MERKLE: Hash update for root entity"
            );
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
