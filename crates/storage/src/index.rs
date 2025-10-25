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
use crate::interface::StorageError;
use crate::store::{Key, StorageAdaptor};

/// Stored index information for an entity in the storage system.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct EntityIndex {
    /// Unique identifier of the entity.
    id: Id,

    /// Identifier of the parent entity, if any.
    parent_id: Option<Id>,

    /// Information about the child entities, including their [`Id`]s and Merkle
    /// hashes, organised by collection name.
    children: BTreeMap<String, Vec<ChildInfo>>,

    /// Full Merkle hash (entity + descendants). Used to detect any changes in subtree.
    full_hash: [u8; 32],

    /// Own Merkle hash (entity data only). Used to determine if entity itself changed
    /// vs. just its children, enabling bandwidth optimization during sync.
    own_hash: [u8; 32],

    /// Metadata about the entity.
    metadata: Metadata,
}

/// Manages the indexing system for efficient tree navigation.
pub(crate) struct Index<S: StorageAdaptor>(PhantomData<S>);

impl<S: StorageAdaptor> Index<S> {
    /// Adds a child to a collection in the index.
    ///
    /// Most entities will get added in this fashion, as nearly all will have
    /// parents. Only root entities are added without a parent.
    ///
    /// # Parameters
    ///
    /// * `parent_id`  - The [`Id`] of the parent entity.
    /// * `collection` - The name of the collection to which the child is to be
    ///                  added.
    /// * `child`      - The [`ChildInfo`] of the child entity to be added.
    /// * `type_id`    - The type identifier of the entity.
    ///
    /// # Errors
    ///
    /// If there's an issue retrieving or saving the index information, an error
    /// will be returned.
    ///
    /// # See also
    ///
    /// * [`add_root()`](Index::add_root())
    /// * [`remove_child_from()`](Index::remove_child_from())
    ///
    pub(crate) fn add_child_to(
        parent_id: Id,
        collection: &str,
        child: ChildInfo,
    ) -> Result<(), StorageError> {
        let mut parent_index =
            Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

        let mut child_index = Self::get_index(child.id())?.unwrap_or_else(|| EntityIndex {
            id: child.id(),
            parent_id: None,
            children: BTreeMap::new(),
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: child.metadata,
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

    /// Adds an index for a root entity.
    ///
    /// Although entities can be added arbitrarily, adding one without a parent
    /// makes it a root. Therefore, this is named to make that clear.
    ///
    /// # Parameters
    ///
    /// * `root` - The [`Id`] and Merkle hash of the entity to be added.
    ///
    /// # Errors
    ///
    /// If there's an issue retrieving or saving the index information, an error
    /// will be returned.
    ///
    /// # See also
    ///
    /// * [`add_child_to()`](Index::add_child_to())
    ///
    pub(crate) fn add_root(root: ChildInfo) -> Result<(), StorageError> {
        let mut index = Self::get_index(root.id())?.unwrap_or_else(|| EntityIndex {
            id: root.id(),
            parent_id: None,
            children: BTreeMap::new(),
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: root.metadata,
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

    /// Retrieves the ancestors of a given entity.
    ///
    /// Retrieves information about the ancestors of the entity, with their IDs
    /// and hashes. The order is from the immediate parent to the root, so index
    /// zero will be the parent, and the last index will be the root.
    ///
    /// # Parameters
    ///
    /// * `id`  - The [`Id`] of the entity whose ancestors are to be retrieved.
    ///
    /// # Errors
    ///
    /// If there's an issue retrieving or deserialising the index information,
    /// an error will be returned.
    ///
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

    /// Retrieves the metadata of a given entity.
    pub(crate) fn get_metadata(id: Id) -> Result<Option<Metadata>, StorageError> {
        Ok(Self::get_index(id)?.map(|index| index.metadata))
    }

    /// Retrieves the children of a given entity.
    ///
    /// # Parameters
    ///
    /// * `parent_id`  - The [`Id`] of the entity whose children are to be
    ///                  retrieved.
    /// * `collection` - The name of the collection from which to retrieve the
    ///                  children.
    ///
    /// # Errors
    ///
    /// If there's an issue retrieving or deserialising the index information,
    /// an error will be returned.
    ///
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

    /// Retrieves the collection names of a given entity.
    ///
    /// # Parameters
    ///
    /// * `parent_id`  - The [`Id`] of the entity that owns the collections.
    ///
    /// # Errors
    ///
    /// If there's an issue retrieving or deserialising the index information,
    /// an error will be returned.
    ///
    pub(crate) fn get_collection_names_for(parent_id: Id) -> Result<Vec<String>, StorageError> {
        Ok(Self::get_index(parent_id)?
            .iter()
            .flat_map(|e| e.children.keys())
            .cloned()
            .collect())
    }

    /// Retrieves the Merkel hashes of a given entity.
    ///
    /// This function returns a tuple of the "own" hash and the "full" hash of
    /// the entity. The "own" hash is the hash of the entity's immediate data
    /// only, while the "full" hash includes the hashes of its descendants.
    ///
    /// # Parameters
    ///
    /// * `id` - The [`Id`] of the entity whose Merkle hashes are to be
    ///          retrieved.
    ///
    /// # Errors
    ///
    /// If there's an issue retrieving or deserialising the index information,
    /// an error will be returned.
    ///
    #[expect(clippy::type_complexity, reason = "Not too complex")]
    pub(crate) fn get_hashes_for(id: Id) -> Result<Option<([u8; 32], [u8; 32])>, StorageError> {
        Ok(Self::get_index(id)?.map(|index| (index.full_hash, index.own_hash)))
    }

    /// Retrieves the index information for an entity.
    ///
    /// # Parameters
    ///
    /// * `id` - The [`Id`] of the entity whose index information is to be
    ///          retrieved.
    ///
    /// # Errors
    ///
    /// If there's an issue retrieving or deserialising the index information,
    /// an error will be returned.
    ///
    fn get_index(id: Id) -> Result<Option<EntityIndex>, StorageError> {
        match S::storage_read(Key::Index(id)) {
            Some(data) => Ok(Some(
                EntityIndex::try_from_slice(&data).map_err(StorageError::DeserializationError)?,
            )),
            None => Ok(None),
        }
    }

    /// Checks if an index exists for a given entity ID.
    ///
    /// # Parameters
    ///
    /// * `id` - The [`Id`] of the entity to check for an index.
    pub(crate) fn has_index(id: Id) -> bool {
        S::storage_read(Key::Index(id)).is_some()
    }

    /// Retrieves the ID of the parent of a given entity.
    ///
    /// # Parameters
    ///
    /// * `child_id` - The [`Id`] of the entity whose parent is to be retrieved.
    ///
    /// # Errors
    ///
    /// If there's an issue retrieving or deserialising the index information,
    /// an error will be returned.
    ///
    pub(crate) fn get_parent_id(child_id: Id) -> Result<Option<Id>, StorageError> {
        Ok(Self::get_index(child_id)?.and_then(|index| index.parent_id))
    }

    /// Whether the collection has children.
    ///
    /// # Parameters
    ///
    /// * `parent_id`  - The [`Id`] of the parent entity.
    /// * `collection` - The name of the collection to which the child is to be
    ///                  added.
    ///
    /// # Errors
    ///
    /// If there's an issue retrieving or saving the index information, an error
    /// will be returned.
    ///
    pub(crate) fn has_children(parent_id: Id, collection: &str) -> Result<bool, StorageError> {
        let parent_index =
            Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

        Ok(parent_index
            .children
            .get(collection)
            .map_or(false, |children| !children.is_empty()))
    }

    /// Recalculates the Merkle hashes of the ancestors of the entity.
    ///
    /// This function recalculates the Merkle hashes of the ancestors of the
    /// entity with the specified ID. This is done by recalculating the Merkle
    /// hash of the entity's parent, plus its children, and then repeating this
    /// recursively up the hierarchy.
    ///
    /// # Parameters
    ///
    /// * `id` - The ID of the entity whose ancestors' hashes should be updated.
    ///
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

    /// Removes a child from a collection in the index.
    ///
    /// Note that removing a child from the index also deletes the child. To
    /// move a child to a different parent, just add it to the new parent.
    ///
    /// # Parameters
    ///
    /// * `parent_id`  - The [`Id`] of the parent entity.
    /// * `collection` - The name of the collection from which the child is to
    ///                  be removed.
    /// * `child_id`   - The [`Id`] of the child entity to be removed.
    ///
    /// # Errors
    ///
    /// If there's an issue retrieving or saving the index information, an error
    /// will be returned.
    ///
    /// # See also
    ///
    /// * [`add_child_to()`](Index::add_child_to())
    ///
    pub(crate) fn remove_child_from(
        parent_id: Id,
        collection: &str,
        child_id: Id,
    ) -> Result<(), StorageError> {
        let mut parent_index =
            Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

        if let Some(children) = parent_index.children.get_mut(collection) {
            children.retain(|child| child.id() != child_id);
        }

        parent_index.full_hash =
            Self::calculate_full_hash_from(parent_index.own_hash, &parent_index.children, false)?;
        Self::save_index(&parent_index)?;

        Self::remove_index(child_id);

        Self::recalculate_ancestor_hashes_for(parent_id)?;
        Ok(())
    }

    /// Removes the index information for an entity.
    ///
    /// # Parameters
    ///
    /// * `index` - The [`EntityIndex`] to be saved.
    ///
    fn remove_index(id: Id) {
        _ = S::storage_remove(Key::Index(id));
    }

    /// Saves the index information for an entity.
    ///
    /// # Parameters
    ///
    /// * `index` - The [`EntityIndex`] to be saved.
    ///
    /// # Errors
    ///
    /// If there's an issue with serialisation, an error will be returned.
    ///
    fn save_index(index: &EntityIndex) -> Result<(), StorageError> {
        _ = S::storage_write(
            Key::Index(index.id),
            &to_vec(index).map_err(StorageError::SerializationError)?,
        );
        Ok(())
    }

    /// Updates the Merkle hash for an indexed entity.
    ///
    /// This accepts the Merkle hash for the entity's "own" hash only, i.e. not
    /// including descendants. The "full" hash including those descendants is
    /// then calculated and returned.
    ///
    /// # Parameters
    ///
    /// * `id`          - The [`Id`] of the entity being updated.
    /// * `merkle_hash` - The new Merkle hash for the entity.
    ///
    /// # Errors
    ///
    /// If there's an issue updating or saving the index, an error will be
    /// returned.
    ///
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
}
