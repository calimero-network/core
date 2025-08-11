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
use core::marker::PhantomData;
use std::collections::BTreeMap;
use std::io::Error as IoError;

use borsh::{from_slice, to_vec, BorshDeserialize, BorshSerialize};
use eyre::Report;
use indexmap::IndexMap;
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error as ThisError;

use crate::address::{Id, Path};
use crate::entities::{ChildInfo, Collection, Data, Metadata};
use crate::index::Index;
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::sync;

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
    Delete {
        /// Unique identifier of the entity.
        id: Id,

        /// Details of the ancestors of the entity.
        ancestors: Vec<ChildInfo>,
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
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ComparisonData {
    /// The unique identifier of the entity being compared.
    id: Id,

    /// The Merkle hash of the entity's own data, without any descendants.
    own_hash: [u8; 32],

    /// The Merkle hash of the entity's complete data, including child hashes.
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

/// The primary interface for the storage system.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct Interface<S: StorageAdaptor = MainStorage>(PhantomData<S>);

impl<S: StorageAdaptor> Interface<S> {
    /// Adds a child to a collection.
    ///
    /// # Parameters
    ///
    /// * `parent_id`  - The ID of the parent entity that owns the
    ///                  [`Collection`].
    /// * `collection` - The [`Collection`] to which the child should be added.
    /// * `child`      - The child entity to add.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
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
    /// After applying the [`Action`], the ancestor hashes will be recalculated,
    /// and this function will compare them against the expected hashes. If any
    /// of the hashes do not match, the ID of the first entity with a mismatched
    /// hash will be returned — i.e. the nearest ancestor.
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
                // todo! remove_child_from here
                let _ignored = S::storage_remove(Key::Entry(id));
            }
        };

        Ok(())
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
    /// * `parent_id`  - The ID of the parent entity that owns the
    ///                  [`Collection`].
    /// * `collection` - The [`Collection`] to get the children of.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
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

    /// The basic info for children of the [`Collection`].
    ///
    /// This gets basic info for children of the [`Collection`], which are the
    /// [`Element`](crate::entities::Element)s that are directly below the
    /// [`Collection`]'s owner in the hierarchy. This is a simple method that
    /// returns the children as a list, and does not provide any filtering or
    /// ordering.
    ///
    /// See [`children_of()`](Interface::children_of()) for more information.
    ///
    /// # Parameters
    ///
    /// * `parent_id`  - The ID of the parent entity that owns the
    ///                  [`Collection`].
    /// * `collection` - The [`Collection`] to get the children of.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    /// # See also
    ///
    /// [`children_of()`](Interface::children_of())
    ///
    pub fn child_info_for<C: Collection>(
        parent_id: Id,
        collection: &C,
    ) -> Result<Vec<ChildInfo>, StorageError> {
        <Index<S>>::get_children_of(parent_id, collection.name())
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

    /// Compares a foreign entity with a local one, and applies the resulting
    /// actions to bring the two entities into sync.
    ///
    /// # Errors
    ///
    /// This function will return an error if there are issues accessing local
    /// data or if there are problems during the comparison process.
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
    pub fn find_by_id_raw(id: Id) -> Option<Vec<u8>> {
        S::storage_read(Key::Entry(id))
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
    /// * `parent_id`  - The unique identifier of the [`Element`](crate::entities::Element)
    ///                  to find the children of.
    /// * `collection` - The name of the [`Collection`] to find the children of.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
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

    /// Generates comparison data for an entity.
    ///
    /// This function generates comparison data for the specified entity, which
    /// includes the entity's own hash, the full hash of the entity and its
    /// children, and the IDs and hashes of the children themselves.
    ///
    /// # Parameters
    ///
    /// * `entity` - The entity to generate comparison data for.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
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

    /// Whether the [`Collection`] has children.
    ///
    /// This checks whether the [`Collection`] of the specified entity has
    /// children, which are the entities that are directly below the entity's
    /// [`Collection`] in the hierarchy.
    ///
    /// # Parameters
    ///
    /// * `parent_id`   - The unique identifier of the entity to check for
    ///                  children.
    /// * `collection` - The [`Collection`] to check for children.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn has_children<C: Collection>(
        parent_id: Id,
        collection: &C,
    ) -> Result<bool, StorageError> {
        <Index<S>>::has_children(parent_id, collection.name())
    }

    /// Retrieves the parent entity of a given entity.
    ///
    /// # Parameters
    ///
    /// * `child_id` - The [`Id`] of the entity whose parent is to be retrieved.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn parent_of<D: Data>(child_id: Id) -> Result<Option<D>, StorageError> {
        <Index<S>>::get_parent_id(child_id)?
            .map_or_else(|| Ok(None), |parent_id| Self::find_by_id(parent_id))
    }

    /// Removes a child from a collection.
    ///
    /// # Parameters
    ///
    /// * `parent_id`  - The ID of the parent entity that owns the
    ///                  [`Collection`].
    /// * `collection` - The collection from which the child should be removed.
    /// * `child_id`   - The ID of the child entity to remove.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
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

    /// Retrieves the root entity for a given context.
    ///
    /// # Parameters
    ///
    /// * `context_id` - An identifier for the context whose root is to be retrieved.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn root<D: Data>() -> Result<Option<D>, StorageError> {
        Self::find_by_id(Id::root())
    }

    /// Saves the root entity to the storage system, and commits any recorded
    /// actions or comparisons.
    ///
    /// This function must only be called once otherwise it will panic.
    ///
    /// # Errors
    ///
    /// This function will return an error if there are issues accessing local
    /// data or if there are problems during the comparison process.
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
    /// # Hierarchy
    ///
    /// It's important to be aware of the hierarchy when saving an entity. If
    /// the entity is a child, it must have a parent, and the parent must exist
    /// in the storage system. If the parent does not exist, the child will not
    /// be saved, and an error will be returned.
    ///
    /// If the entity is a root, it will be saved as a root, and the parent ID
    /// will be set to [`None`].
    ///
    /// When creating a new child entity, it's important to call
    /// [`add_child_to()`](Interface::add_child_to()) in order to create and
    /// associate the child with the parent. Thereafter, [`save()`](Interface::save())
    /// can be called to save updates to the child entity.
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
    /// * `element` - The [`Element`](crate::entities::Element) whose data
    ///               should be saved. This will be serialised and stored in the
    ///               storage system.
    ///
    /// # Errors
    ///
    /// If an error occurs when serialising data or interacting with the storage
    /// system, an error will be returned.
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

    /// Saves raw data to the storage system.
    ///
    /// # Errors
    ///
    /// If an error occurs when serialising data or interacting with the storage
    /// system, an error will be returned.
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

    /// An attempt was made to create an orphan, i.e. an entity that has not
    /// been registered as either a root or having a parent. This was probably
    /// cause by calling `save()` without calling `add_child_to()` first.
    #[error("Cannot create orphan with ID: {0}")]
    CannotCreateOrphan(Id),

    /// An error occurred during serialization.
    #[error("Deserialization error: {0}")]
    DeserializationError(IoError),

    /// An error occurred when handling threads or async tasks.
    #[error("Dispatch error: {0}")]
    DispatchError(String),

    /// The ID of the entity supplied does not match the ID in the accompanying
    /// comparison data.
    #[error("ID mismatch in comparison data for ID: {0}")]
    IdentifierMismatch(Id),

    /// TODO: An error during tree validation.
    #[error("Invalid data was found for ID: {0}")]
    InvalidDataFound(Id),

    /// An index entry already exists for the specified entity. This would
    /// indicate a bug in the system.
    #[error("Index already exists for ID: {0}")]
    IndexAlreadyExists(Id),

    /// An index entry was not found for the specified entity. This would
    /// indicate a bug in the system.
    #[error("Index not found for ID: {0}")]
    IndexNotFound(Id),

    /// The requested record was not found, but in the context it was asked for,
    /// it was expected to be found and so this represents an error or some kind
    /// of inconsistency in the stored data.
    #[error("Record not found with ID: {0}")]
    NotFound(Id),

    /// An unexpected ID was encountered.
    #[error("Unexpected ID: {0}")]
    UnexpectedId(Id),

    /// An error occurred during serialization.
    #[error("Serialization error: {0}")]
    SerializationError(IoError),

    /// TODO: An error from the Store.
    #[error("Store error: {0}")]
    StoreError(#[from] Report),

    /// An unknown collection type was specified.
    #[error("Unknown collection type: {0}")]
    UnknownCollectionType(String),

    /// An unknown type was specified.
    #[error("Unknown type: {0}")]
    UnknownType(u8),
}

impl Serialize for StorageError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            Self::ActionNotAllowed(ref err)
            | Self::DispatchError(ref err)
            | Self::UnknownCollectionType(ref err) => serializer.serialize_str(err),
            Self::DeserializationError(ref err) | Self::SerializationError(ref err) => {
                serializer.serialize_str(&err.to_string())
            }
            Self::CannotCreateOrphan(id)
            | Self::IndexAlreadyExists(id)
            | Self::IndexNotFound(id)
            | Self::IdentifierMismatch(id)
            | Self::InvalidDataFound(id)
            | Self::UnexpectedId(id)
            | Self::NotFound(id) => serializer.serialize_str(&id.to_string()),
            Self::StoreError(ref err) => serializer.serialize_str(&err.to_string()),
            Self::UnknownType(err) => serializer.serialize_u8(err),
        }
    }
}
