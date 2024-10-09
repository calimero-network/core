//! Interface for the storage system.
//!
//! This module contains the interface for the storage system, which provides
//! the basics of loading and saving data, but presents a number of helper
//! methods and additional functionality to abstract away common operations.
//!
//! This follows the repository pattern, where the interface is the primary
//! means of interacting with the storage system, rather than the ActiveRecord
//! pattern where the model is the primary means of interaction.
//!
//! # Synchronisation
//!
//! There are two main mechanisms involved in synchronisation:
//!
//!   1. **Direct changes**: When a change is made locally, the resulting
//!      actions need to be propagated to other nodes.
//!   2. **Comparison**: When a comparison is made between two nodes, the
//!      resulting actions need to be taken to bring the nodes into sync.
//!
//! The entry points for synchronisation are therefore either the
//! [`apply_action()`](Interface::apply_action()) method, to carry out actions;
//! or the [`compare_trees()`](Interface::compare_trees()) method, to compare
//! two nodes, which will emit actions to pass to [`apply_action()`](Interface::apply_action())
//! on either the local or remote node, or both.
//!
//! ## CRDT model
//!
//! Calimero primarily uses operation-based CRDTs, also called commutative
//! replicated data types (CmRDTs). This means that the order of operations does
//! not matter, and the outcome will be the same regardless of the order in
//! which the operations are applied.
//!
//! It is worth noting that the orthodox CmRDT model does not feature or require
//! a comparison activity, as there is an operating assumption that all updates
//! have been carried out fully and reliably.
//!
//! The alternative CRDT approach is state-based, also called convergent
//! replicated data types (CvRDTs). This is a comparison-based approach, but the
//! downside is that this model requires the full state to be transmitted
//! between nodes, which can be costly. Although this approach is easier to
//! implement, and fits well with gossip protocols, it is not as efficient as
//! the operation-based model.
//!
//! The standard choice therefore comes down to:
//!
//!   - Use CmRDTs and guarantee that all updates are carried out fully and
//!     reliably, are not dropped or duplicated, and are replayed in causal
//!     order.
//!   - Use CvRDTs and accept the additional bandwidth cost of transmitting the
//!     full state for every single CRDT.
//!
//! It does not fit the Calimero objectives to transmit the entire state for
//! every update, but there is also no guarantee that all updates will be
//! carried out fully and reliably. Therefore, Calimero uses a hybrid approach
//! that represents the best of both worlds.
//!
//! In the first instance, operations are emitted (in the form of [`Action`]s)
//! whenever a change is made. These operations are then propagated to other
//! nodes, where they are executed. This is the CmRDT model.
//!
//! However, there are cases where a node may have missed an update, for
//! instance, if it was offline. In this case, the node will be out of sync with
//! the rest of the network. To bring the node back into sync, a comparison is
//! made between the local node and a remote node, and the resulting actions are
//! executed. This is the CvRDT model.
//!
//! The storage system maintains a set of Merkle hashes, which are stored
//! against each element, and represent the state of the element and its
//! children. The Merkle hash for an element can therefore be used to trivially
//! determine whether an element or any of its descendants have changed,
//! without actually needing to compare every entity in the tree.
//!
//! Therefore, when a comparison is carried out it is not a full state
//! comparison, but a comparison of the immediate data and metadata of given
//! element(s). This is sufficient to determine whether the nodes are in sync,
//! and to generate the actions needed to bring them into sync. If there is any
//! deviation detected, the comparison will recurse into the children of the
//! element(s) in question, and generate further actions as necessary — but this
//! will only ever examine those descendent entities for which the Merkle hash
//! differs.
//!
//! We can therefore summarise this position as being: Calimero uses a CmRDT
//! model for direct changes, and a CvRDT model for comparison as a fallback
//! mechanism to bring nodes back into sync when needed.
//!
//! ## Direct changes
//!
//! When a change is made locally, the resulting actions need to be propagated
//! to other nodes. An action list will be generated, which can be made up of
//! [`Add`](Action::Add), [`Delete`](Action::Delete), and [`Update`](Action::Update)
//! actions. These actions are then propagated to all the other nodes in the
//! network, where they are executed using the [`apply_action()`](Interface::apply_action())
//! method.
//!
//! This is a straightforward process, as the actions are known and are fully
//! self-contained without any wider impact. Order does not strictly matter, as
//! the actions are commutative, and the outcome will be the same regardless of
//! the order in which they are applied. Any conflicts are handled using the
//! last-write-wins strategy.
//!
//! There are certain cases where a mis-ordering of action, which is
//! essentially the same as having missing actions, can result in an invalid
//! state. For instance, if a child is added before the parent, the parent will
//! not exist and the child will be orphaned. In this situation we can either
//! ignore the child, or we can block its addition until the parent has been
//! added, or we can store it as an orphaned entity to be resolved later. At
//! present we follow the last approach, as it aligns well with the use of
//! comparisons to bring nodes back into sync. We therefore know that the node
//! will _eventually_ become consistent, which is all we guarantee.
//!
//! TODO: Examine whether this is the right approach, or whether we should for
//! TODO: instance block and do a comparison on the parent to ensure that local
//! TODO: state is as consistent as possible.
//!
//! Providing all generated actions are carried out, all nodes will eventually
//! be in sync, without any need for comparisons, transmission of full states,
//! or a transaction model (which requires linear history, and therefore
//! becomes mathematically unsuitable for large distributed systems).
//!
//! ## Comparison
//!
//! There are a number of situations under which a comparison may be needed:
//!
//!   1. A node has missed an update, and needs to be brought back into sync
//!      (i.e. there is a gap in the instruction set).
//!   2. A node has been offline, and needs to be brought back into sync (i.e.
//!      all instructions since a certain point have been missed).
//!   3. A discrepancy has been detected between two nodes, and they need to be
//!      brought back into sync.
//!
//! A comparison is primarily triggered from a catch-up as a proactive measure,
//! i.e. without knowing if any changes exist, but can also arise at any point
//! if a discrepancy is detected.
//!
//! When performing a comparison, the data we have is the result of the entity
//! being serialised by the remote note, passed to us, and deserialised, so it
//! should be comparable in structure to having loaded it from the local
//! database.
//!
//! We therefore have access to the data and metadata, which includes the
//! immediate fields of the entity (i.e. the [`AtomicUnit`](crate::entities::AtomicUnit))
//! and also a list of the children.
//!
//! The stored list of children contains their Merkle hashes, thereby allowing
//! us to check all children in one operation instead of needing to load each
//! child in turn, as that would require remote data for each child, and that
//! would not be as efficient.
//!
//!   - If a child exists on both sides and the hash is different, we recurse
//!     into a comparison for that child.
//!
//!   - If a child is missing on one side then we can go with the side that has
//!     the latest parent and add or remove the child.
//!
//! Notably, in the case of there being a missing child, the resolution
//! mechanism does expose a small risk of erroneous outcome. For instance, if
//! the local node has had a child added, and has been offline, and the parent
//! has been updated remotely — in this situation, in the absence of any other
//! activity, a comparison (e.g. a catch-up when coming back online) would
//! result in losing the child, as the remote node would not have the child in
//! its list of children. This edge case should usually be handled by the
//! specific add and remove events generated at the time of the original
//! activity, which should get propagated independently of a sync. However,
//! there are three extended options that can be implemented:
//!
//!   1. Upon catch-up, any local list of actions can be held and replayed
//!      locally after synchronisation. Alone, this would not correct the
//!      situation, due to last-write-wins rules, but additional logic could be
//!      considered for this particular situation.
//!   2. During or after performing a comparison, all local children for an
//!      entity can be loaded by path and checked against the parent's list of
//!      children. If there are any deviations then appropriate action can be
//!      taken. This does not fully cater for the edge case, and again would not
//!      correct the situation on its own, but additional logic could be added.
//!   3. A special kind of "deleted" element could be added to the system, which
//!      would store metadata for checking. This would be beneficial as it would
//!      allow differentiation between a missing child and a deleted child,
//!      which is the main problem exposed by the edge case. However, although
//!      this represents a full solution from a data mechanism perspective, it
//!      would not be desirable to store deleted entries permanently. It would
//!      be best combined with a cut-off constraint, that would limit the time
//!      period in which a catch-up can be performed, and after which the
//!      deleted entities would be purged. This does add complexity not only in
//!      the effect on the wider system of implementing that constraint, but
//!      also in the need for garbage collection to remove the deleted entities.
//!      This would likely be better conducted the next time the parent entity
//!      is updated, but there are a number of factors to consider here.
//!   4. Another way to potentially handle situations of this nature is to
//!      combine multiple granular updates into an atomic group operation that
//!      ensures that all updates are applied together. However, this remains to
//!      be explored, as it may not fit with the wider system design.
//!
//! Due to the potential complexity, this edge case is not currently mitigated,
//! but will be the focus of future development.
//!
//! TODO: Assess the best approach for handling this edge case in a way that
//! TODO: fits with the wider system design, and add extended tests for it.
//!
//! The outcome of a comparison is that the calling code receives a list of
//! actions, which can be [`Add`](Action::Add), [`Delete`](Action::Delete),
//! [`Update`](Action::Update), and [`Compare`](Action::Compare). The first
//! three are the same as propagating the consequences of direct changes, but
//! the comparison action is a special case that arises when a child entity is
//! found to differ between the two nodes, whereupon the comparison process
//! needs to recursively descend into the parts of the subtree found to differ.
//!
//! The calling code is then responsible for going away and obtaining the
//! information necessary to carry out the next comparison action if there is
//! one, as well as relaying the generated action list.
//!

#[cfg(test)]
#[path = "tests/interface.rs"]
mod tests;

use core::fmt::Debug;
use std::io::Error as IoError;

use borsh::{to_vec, BorshDeserialize, BorshSerialize};
use calimero_sdk::env;
use env::{storage_read, storage_remove, storage_write};
use eyre::Report;
use indexmap::IndexMap;
use sha2::{Digest, Sha256};
use thiserror::Error as ThisError;

use crate::address::{Id, Path};
use crate::entities::{Collection, Data};

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
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[expect(clippy::exhaustive_enums, reason = "Exhaustive")]
pub enum Action {
    /// Add an entity with the given ID.
    Add(Id, Vec<u8>),

    /// Compare the given entity. Note that this results in a direct comparison
    /// of the specific entity in question, including data that is immediately
    /// available to it, such as the hashes of its children. This may well
    /// result in further actions being generated if children differ, leading to
    /// a recursive comparison.
    Compare(Id),

    /// Delete an entity with the given ID.
    Delete(Id),

    /// Update the entity with the given ID.
    Update(Id, Vec<u8>),
}

/// The primary interface for the storage system.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct Interface;

impl Interface {
    /// Applies an [`Action`] to the storage system.
    ///
    /// This function accepts a single incoming [`Action`] and applies it to the
    /// storage system. The action is deserialised into the specified type, and
    /// then applied as appropriate.
    ///
    /// Note: In order to call this function, the caller needs to know the type
    /// of the entity that the action is for, and this type must implement the
    /// [`Data`] trait. One of the possible ways to achieve this is to use the
    /// path information externally to match against the entity type, using
    /// knowledge available in the system. It is also possible to extend this
    /// function to deal with the type being indicated in the serialised data,
    /// if appropriate, or in the ID or accompanying metadata.
    ///
    /// TODO: Establish whether any additional data encoding is needed, to help
    /// TODO: with deserialisation.
    ///
    /// # Parameters
    ///
    /// * `action` - The [`Action`] to apply to the storage system.
    ///
    /// # Errors
    ///
    /// If there is an error when deserialising into the specified type, or when
    /// applying the [`Action`], an error will be returned.
    ///
    pub fn apply_action<D: Data>(action: Action) -> Result<(), StorageError> {
        match action {
            Action::Add(id, serialized_data) | Action::Update(id, serialized_data) => {
                let mut entity = D::try_from_slice(&serialized_data)
                    .map_err(StorageError::DeserializationError)?;
                _ = Self::save(id, &mut entity)?;
            }
            Action::Compare(_) => return Err(StorageError::ActionNotAllowed("Compare".to_owned())),
            Action::Delete(id) => {
                _ = storage_remove(id.to_string().as_bytes());
            }
        }
        Ok(())
    }

    /// Calculates the Merkle hash for the [`Element`](crate::entities::Element).
    ///
    /// This calculates the Merkle hash for the
    /// [`Element`](crate::entities::Element), which is a cryptographic hash of
    /// the significant data in the "scope" of the [`Element`](crate::entities::Element),
    /// and is used to determine whether the data has changed and is valid. It
    /// is calculated by hashing the substantive data in the [`Element`](crate::entities::Element),
    /// along with the hashes of the children of the [`Element`](crate::entities::Element),
    /// thereby representing the state of the entire hierarchy below the
    /// [`Element`](crate::entities::Element).
    ///
    /// This method is called automatically when the [`Element`](crate::entities::Element)
    /// is updated, but can also be called manually if required.
    ///
    /// # Significant data
    ///
    /// The data considered "significant" to the state of the [`Element`](crate::entities::Element),
    /// and any change to which is considered to constitute a change in the
    /// state of the [`Element`](crate::entities::Element), is:
    ///
    ///   - The ID of the [`Element`](crate::entities::Element). This should
    ///     never change. Arguably, this could be omitted, but at present it
    ///     means that empty elements are given meaningful hashes.
    ///   - The primary [`Data`] of the [`Element`](crate::entities::Element).
    ///     This is the data that the consumer application has stored in the
    ///     [`Element`](crate::entities::Element), and is the focus of the
    ///     [`Element`](crate::entities::Element).
    ///   - The metadata of the [`Element`](crate::entities::Element). This is
    ///     the system-managed properties that are used to process the
    ///     [`Element`](crate::entities::Element), but are not part of the
    ///     primary data. Arguably the Merkle hash could be considered part of
    ///     the metadata, but it is not included in the [`Data`] struct at
    ///     present (as it obviously should not contribute to the hash, i.e.
    ///     itself).
    ///
    /// Note that private data is not considered significant, as it is not part
    /// of the shared state, and therefore does not contribute to the hash.
    ///
    /// # Parameters
    ///
    /// * `element`     - The [`Element`](crate::entities::Element) to calculate
    ///                   the Merkle hash for.
    /// * `recalculate` - Whether to recalculate or use the cached value for
    ///                   child hashes. Under normal circumstances, the cached
    ///                   value should be used, as it is more efficient. The
    ///                   option to recalculate is provided for situations when
    ///                   the entire subtree needs revalidating.
    ///
    /// # Errors
    ///
    /// If there is a problem in serialising the data, an error will be
    /// returned.
    ///
    pub fn calculate_merkle_hash_for<D: Data>(
        entity: &D,
        recalculate: bool,
    ) -> Result<[u8; 32], StorageError> {
        let mut hasher = Sha256::new();
        hasher.update(entity.calculate_merkle_hash()?);

        for (collection_name, children) in entity.collections() {
            for child_info in children {
                let child_hash = if recalculate {
                    let child_data = Self::find_by_id_raw(child_info.id())?
                        .ok_or_else(|| StorageError::NotFound(child_info.id()))?;
                    entity.calculate_merkle_hash_for_child(&collection_name, &child_data)?
                } else {
                    child_info.merkle_hash()
                };
                hasher.update(child_hash);
            }
        }

        Ok(hasher.finalize().into())
    }

    /// The children of the [`Collection`].
    ///
    /// This gets the children of the [`Collection`], which are the [`Element`](crate::entities::Element)s
    /// that are directly below the [`Collection`]'s owner in the hierarchy.
    /// This is a simple method that returns the children as a list, and does
    /// not provide any filtering or ordering.
    ///
    /// Notably, there is no real concept of ordering in the storage system, as
    /// the records are not ordered in any way. They are simply stored in the
    /// hierarchy, and so the order of the children is not guaranteed. Any
    /// required ordering must be done as required upon retrieval.
    ///
    /// # Determinism
    ///
    /// TODO: Update when the `child_info` field is replaced with an index.
    ///
    /// Depending on the source, simply looping through the children may be
    /// non-deterministic. At present we are using a [`Vec`], which is
    /// deterministic, but this is a temporary measure, and the order of
    /// children under a given path is not enforced, and therefore
    /// non-deterministic.
    ///
    /// When the `child_info` field is replaced with an index, the order may be
    /// enforced using `created_at` timestamp, which then allows performance
    /// optimisations with sharding and other techniques.
    ///
    /// # Performance
    ///
    /// TODO: Update when the `child_info` field is replaced with an index.
    ///
    /// Looping through children and combining their hashes into the parent is
    /// logically correct. However, it does necessitate loading all the children
    /// to get their hashes every time there is an update. The efficiency of
    /// this can and will be improved in future.
    ///
    /// # Parameters
    ///
    /// * `collection` - The [`Collection`] to get the children of.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn children_of<C: Collection>(collection: &C) -> Result<Vec<C::Child>, StorageError> {
        let mut children = Vec::new();
        for info in collection.child_info() {
            children.push(Self::find_by_id(info.id())?.ok_or(StorageError::NotFound(info.id()))?);
        }
        Ok(children)
    }

    /// Compares a foreign entity with a local one.
    ///
    /// This function compares a foreign entity, usually from a remote node,
    /// with the version available in the tree in local storage, if present, and
    /// generates a list of [`Action`]s to perform on the local tree, the
    /// foreign tree, or both, to bring the two trees into sync.
    ///
    /// The tuple returned is composed of two lists of actions: the first list
    /// contains the actions to be performed on the local tree, and the second
    /// list contains the actions to be performed on the foreign tree.
    ///
    /// # Parameters
    ///
    /// * `foreign_entity` - The foreign entity to compare against the local
    ///                      version. This will usually be from a remote node.
    ///
    /// # Errors
    ///
    /// This function will return an error if there are issues accessing local
    /// data or if there are problems during the comparison process.
    ///
    pub fn compare_trees<D: Data>(
        foreign_entity: &D,
    ) -> Result<(Vec<Action>, Vec<Action>), StorageError> {
        let mut actions = (vec![], vec![]);
        let Some(local_entity) = Self::find_by_id::<D>(foreign_entity.id())? else {
            // Local entity doesn't exist, so we need to add it
            actions.0.push(Action::Add(
                foreign_entity.id(),
                to_vec(foreign_entity).map_err(StorageError::SerializationError)?,
            ));
            return Ok(actions);
        };

        if local_entity.element().merkle_hash() == foreign_entity.element().merkle_hash() {
            return Ok(actions);
        }
        if local_entity.element().updated_at() <= foreign_entity.element().updated_at() {
            actions.0.push(Action::Update(
                local_entity.id(),
                to_vec(foreign_entity).map_err(StorageError::SerializationError)?,
            ));
        } else {
            actions.1.push(Action::Update(
                foreign_entity.id(),
                to_vec(&local_entity).map_err(StorageError::SerializationError)?,
            ));
        }

        let local_collections = local_entity.collections();
        let foreign_collections = foreign_entity.collections();

        for (local_coll_name, local_children) in &local_collections {
            if let Some(foreign_children) = foreign_collections.get(local_coll_name) {
                let local_child_map: IndexMap<_, _> = local_children
                    .iter()
                    .map(|child| (child.id(), child.merkle_hash()))
                    .collect();
                let foreign_child_map: IndexMap<_, _> = foreign_children
                    .iter()
                    .map(|child| (child.id(), child.merkle_hash()))
                    .collect();

                for (id, local_hash) in &local_child_map {
                    match foreign_child_map.get(id) {
                        Some(foreign_hash) if local_hash != foreign_hash => {
                            actions.0.push(Action::Compare(*id));
                            actions.1.push(Action::Compare(*id));
                        }
                        None => {
                            // We need to fetch the child entity and serialize it
                            if let Some(local_child) = Self::find_by_id_raw(*id)? {
                                actions.1.push(Action::Add(*id, local_child));
                            }
                        }
                        // Hashes match, no action needed
                        _ => {}
                    }
                }

                for id in foreign_child_map.keys() {
                    if !local_child_map.contains_key(id) {
                        // We can't get the full data for the foreign child, so we flag it for comparison
                        actions.1.push(Action::Compare(*id));
                    }
                }
            } else {
                // The entire collection is missing from the foreign entity
                for child in local_children {
                    if let Some(local_child) = Self::find_by_id_raw(child.id())? {
                        actions.1.push(Action::Add(child.id(), local_child));
                    }
                }
            }
        }

        // Check for collections in the foreign entity that don't exist locally
        for (foreign_coll_name, foreign_children) in &foreign_collections {
            if !local_collections.contains_key(foreign_coll_name) {
                for child in foreign_children {
                    // We can't get the full data for the foreign child, so we flag it for comparison
                    actions.1.push(Action::Compare(child.id()));
                }
            }
        }

        Ok(actions)
    }

    /// Finds an [`Element`](crate::entities::Element) by its unique identifier.
    ///
    /// This will always retrieve a single [`Element`](crate::entities::Element),
    /// if it exists, regardless of where it may be in the hierarchy, or what
    /// state it may be in.
    ///
    /// # Parameters
    ///
    /// * `id` - The unique identifier of the [`Element`](crate::entities::Element)
    ///          to find.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn find_by_id<D: Data>(id: Id) -> Result<Option<D>, StorageError> {
        let value = storage_read(id.to_string().as_bytes());

        match value {
            Some(slice) => {
                let mut entity =
                    D::try_from_slice(&slice).map_err(StorageError::DeserializationError)?;
                // TODO: This is needed for now, as the field gets stored. Later we will
                // TODO: implement a custom serialiser that will skip this field along with
                // TODO: any others that should not be stored.
                entity.element_mut().is_dirty = false;
                Ok(Some(entity))
            }
            None => Ok(None),
        }
    }

    /// Finds an [`Element`](crate::entities::Element) by its unique identifier
    /// without deserialising it.
    ///
    /// This will always retrieve a single [`Element`](crate::entities::Element),
    /// if it exists, regardless of where it may be in the hierarchy, or what
    /// state it may be in.
    ///
    /// Notably it returns the raw bytes without attempting to deserialise them
    /// into a [`Data`] type.
    ///
    /// # Parameters
    ///
    /// * `id` - The unique identifier of the [`Element`](crate::entities::Element)
    ///          to find.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn find_by_id_raw(id: Id) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(storage_read(id.to_string().as_bytes()))
    }

    /// Finds one or more [`Element`](crate::entities::Element)s by path in the
    /// hierarchy.
    ///
    /// This will retrieve all [`Element`](crate::entities::Element)s that exist
    /// at the specified path in the hierarchy. This may be a single item, or
    /// multiple items if there are multiple [`Element`](crate::entities::Element)s
    /// at the same path.
    ///
    /// # Parameters
    ///
    /// * `path` - The path to the [`Element`](crate::entities::Element)s to
    ///            find.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn find_by_path<D: Data>(_path: &Path) -> Result<Vec<D>, StorageError> {
        unimplemented!()
    }

    /// Finds the children of an [`Element`](crate::entities::Element) by its
    /// unique identifier.
    ///
    /// This will retrieve all [`Element`](crate::entities::Element)s that are
    /// children of the specified [`Element`](crate::entities::Element). This
    /// may be a single item, or multiple items if there are multiple children.
    /// Notably, it will return [`None`] if the [`Element`](crate::entities::Element)
    /// in question does not exist.
    ///
    /// # Parameters
    ///
    /// * `id` - The unique identifier of the [`Element`](crate::entities::Element)
    ///          to find the children of.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn find_children_by_id<D: Data>(_id: Id) -> Result<Option<Vec<D>>, StorageError> {
        unimplemented!()
    }

    /// Saves an [`Element`](crate::entities::Element) to the storage system.
    ///
    /// This will save the provided [`Element`](crate::entities::Element) to the
    /// storage system. If the record already exists, it will be updated with
    /// the new data. If the record does not exist, it will be created.
    ///
    /// # Update guard
    ///
    /// If the provided [`Element`](crate::entities::Element) is older than the
    /// existing record, the update will be ignored, and the existing record
    /// will be kept. The Boolean return value indicates whether the record was
    /// saved or not; a value of `false` indicates that the record was not saved
    /// due to this guard check — any other reason will be due to an error, and
    /// returned as such.
    ///
    /// # Dirty flag
    ///
    /// Note, if the [`Element`](crate::entities::Element) is not marked as
    /// dirty, it will not be saved, but `true` will be returned. In this case,
    /// the record is considered to be up-to-date and does not need saving, and
    /// so the save operation is effectively a no-op. If necessary, this can be
    /// checked before calling [`save()](crate::entities::Element::save()) by
    /// calling [`is_dirty()](crate::entities::Element::is_dirty()).
    ///
    /// # Merkle hash
    ///
    /// The Merkle hash of the [`Element`](crate::entities::Element) is
    /// calculated before saving, and stored in the [`Element`](crate::entities::Element)
    /// itself. This is used to determine whether the data of the [`Element`](crate::entities::Element)
    /// or its children has changed, and is used to validate the stored data.
    ///
    /// Note that if the [`Element`](crate::entities::Element) does not need
    /// saving, or cannot be saved, then the Merkle hash will not be updated.
    /// This way the hash only ever represents the state of the data that is
    /// actually stored.
    ///
    /// # Parameters
    ///
    /// * `id`      - The unique identifier of the [`Element`](crate::entities::Element)
    ///               to save.
    /// * `element` - The [`Element`](crate::entities::Element) whose data
    ///               should be saved. This will be serialised and stored in the
    ///               storage system.
    ///
    /// # Errors
    ///
    /// If an error occurs when serialising data or interacting with the storage
    /// system, an error will be returned.
    ///
    pub fn save<D: Data>(id: Id, entity: &mut D) -> Result<bool, StorageError> {
        if !entity.element().is_dirty() {
            return Ok(true);
        }
        // It is possible that the record gets added or updated after the call to
        // this find() method, and before the put() to save the new data... however,
        // this is very unlikely under our current operating model, and so the risk
        // is considered acceptable. If this becomes a problem, we should change
        // the RwLock to a ReentrantMutex, or reimplement the get() logic here to
        // occur within the write lock. But this seems unnecessary at present.
        if let Some(mut existing) = Self::find_by_id::<D>(id)? {
            if existing.element_mut().metadata.updated_at >= entity.element().metadata.updated_at {
                return Ok(false);
            }
        }
        // TODO: Need to propagate the change up the tree, i.e. trigger a
        // TODO: recalculation for the ancestors.
        entity.element_mut().merkle_hash = Self::calculate_merkle_hash_for(entity, false)?;

        _ = storage_write(
            id.to_string().as_bytes(),
            &to_vec(entity).map_err(StorageError::SerializationError)?,
        );
        entity.element_mut().is_dirty = false;
        Ok(true)
    }

    /// Validates the stored state.
    ///
    /// This will validate the stored state of the storage system, i.e. the data
    /// that has been saved to the storage system, ensuring that it is correct
    /// and consistent. This is done by calculating Merkle hashes of the stored
    /// data, and comparing them to the expected hashes.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn validate() -> Result<(), StorageError> {
        unimplemented!()
    }
}

/// Errors that can occur when working with the storage system.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum StorageError {
    /// The requested action is not allowed.
    #[error("Action not allowed: {0}")]
    ActionNotAllowed(String),

    /// An error occurred during serialization.
    #[error("Deserialization error: {0}")]
    DeserializationError(IoError),

    /// An error occurred when handling threads or async tasks.
    #[error("Dispatch error: {0}")]
    DispatchError(String),

    /// TODO: An error during tree validation.
    #[error("Invalid data was found for ID: {0}")]
    InvalidDataFound(Id),

    /// The requested record was not found, but in the context it was asked for,
    /// it was expected to be found and so this represents an error or some kind
    /// of inconsistency in the stored data.
    #[error("Record not found with ID: {0}")]
    NotFound(Id),

    /// An error occurred during serialization.
    #[error("Serialization error: {0}")]
    SerializationError(IoError),

    /// TODO: An error from the Store.
    #[error("Store error: {0}")]
    StoreError(#[from] Report),

    /// An unknown collection type was specified.
    #[error("Unknown collection type: {0}")]
    UnknownCollectionType(String),
}
