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
use tracing::debug;

use crate::address::Id;
use crate::collections::crdt_meta::CrdtType;
use crate::entities::{ChildInfo, Data, Metadata, SignatureData, StorageType};
use crate::env::time_now;
use crate::index::Index;
use crate::merge::{try_merge_by_type_name, try_merge_registered, WasmMergeCallback};
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
    pub fn add_child_to<D: Data>(parent_id: Id, child: &mut D) -> Result<bool, StorageError> {
        if !child.element().is_dirty() {
            return Ok(false);
        }

        let data = to_vec(child).map_err(|e| StorageError::SerializationError(e.into()))?;

        let own_hash = Sha256::digest(&data).into();

        <Index<S>>::add_child_to(
            parent_id,
            ChildInfo::new(child.id(), own_hash, child.element().metadata.clone()),
        )?;

        let Some(hash) = Self::save_raw(child.id(), data, child.element().metadata.clone())? else {
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
        crate::env::log("apply_action");
        // TODO: refactor to a separate function.
        // Run verification logic before applying
        match &action {
            Action::Add {
                metadata, data, id, ..
            }
            | Action::Update {
                metadata, data, id, ..
            } => {
                Self::verify_action_update(&action)?;

                match &metadata.storage_type {
                    StorageType::User {
                        owner,
                        signature_data,
                    } => {
                        debug!(
                            %id,
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            %owner,
                            ?owner,
                            data_len = data.len(),
                            "Interface::apply_action received upsert user action"
                        );
                        crate::env::log(&format!(
                            "Interface::apply_action received upsert user action: \
                            \n=== Id: {id};\
                            \n=== created_at: {0};  updated_at: {1};\
                            \n=== owner: {owner}; data_len: {2}",
                            metadata.created_at,
                            metadata.updated_at(),
                            data.len()
                        ));

                        let sig_data = signature_data.as_ref().ok_or(StorageError::InvalidData(
                            "Remote User action must be signed".to_owned(),
                        ))?;

                        debug!(
                            %id,
                            ?id,
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            %owner,
                            ?owner,
                            data_len = data.len(),
                            ?sig_data.signature,
                            sig_data.nonce,
                            "Interface::apply_action received upsert user action: sig data"
                        );

                        let payload = action.payload_for_signing();

                        crate::env::log(&format!(
                            "Interface::apply_action received upsert user action: sig data \
                            \n=== Signature: {:?}; signature_len: {}, nonce: {}; \
                            \n=== owner: {}; owner_bytes: {:?} \
                            \n=== payload: {:?}",
                            sig_data.signature,
                            sig_data.signature.len(),
                            sig_data.nonce,
                            owner,
                            owner.digest(),
                            payload,
                        ));
                        crate::env::log("Interface::apply_action received upsert user action: getting last nonce from storage");

                        // Replay protection check
                        let new_nonce = sig_data.nonce;
                        let last_nonce = <Index<S>>::get_metadata(*id)?
                            .map(|m| *m.updated_at)
                            .unwrap_or(0);

                        crate::env::log(&format!(
                            "Interface::apply_action received upsert user action: last nonce from storage \
                            \n=== new_nonce: {}; last_nonce: {}",
                            new_nonce, last_nonce
                        ));

                        if new_nonce <= last_nonce {
                            return Err(StorageError::NonceReplay(Box::new((*owner, new_nonce))));
                        }

                        let verification_result = crate::env::ed25519_verify(
                            &sig_data.signature,
                            owner.digest(),
                            &payload,
                        );

                        crate::env::log(&format!(
                            "Interface::apply_action received upsert user action: verify signature\
                            \n=== Id: {id}; Signature_verification_result: {0}; owner: {owner:?}; owner:{owner}",
                            verification_result,
                        ));

                        if !verification_result {
                            return Err(StorageError::InvalidSignature);
                        }
                    }
                    StorageType::Frozen => {
                        debug!(
                            %id,
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            data_len = data.len(),
                            "Interface::apply_action received upsert frozen action"
                        );
                        verify_frozen_action_upsert(&action, data)?;
                    }
                    StorageType::Public => { /* No special checks */ }
                }
            }
            Action::DeleteRef { id, metadata, .. } => {
                // Get the metadata of the item being deleted to check its domain
                let existing_metadata =
                    <Index<S>>::get_metadata(*id)?.ok_or(StorageError::IndexNotFound(*id))?;

                match existing_metadata.storage_type {
                    StorageType::Frozen => {
                        debug!(
                            %id,
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            "Interface::apply_action received delete frozen action"
                        );
                        return Err(StorageError::ActionNotAllowed(
                            "Frozen data cannot be deleted".to_owned(),
                        ));
                    }
                    StorageType::User {
                        owner: existing_owner,
                        ..
                    } => {
                        // Verify the action's metadata, which contains the signature
                        match &metadata.storage_type {
                            StorageType::User {
                                owner,
                                signature_data,
                            } => {
                                // Check it matches the owner on record
                                if *owner != existing_owner {
                                    return Err(StorageError::InvalidSignature);
                                }

                                let sig_data =
                                    signature_data.as_ref().ok_or(StorageError::InvalidData(
                                        "Remote User delete must be signed".to_owned(),
                                    ))?;

                                // TODO: refactor to a separate function.
                                // Replay protection check
                                let new_nonce = sig_data.nonce;
                                // The nonce is the `deleted_at` time. We check it against the
                                // last `updated_at` time stored in the index.
                                let last_nonce = *existing_metadata.updated_at;

                                if new_nonce <= last_nonce {
                                    return Err(StorageError::NonceReplay(Box::new((
                                        *owner, new_nonce,
                                    ))));
                                }

                                let payload = action.payload_for_signing();
                                let verification_result = crate::env::ed25519_verify(
                                    &sig_data.signature,
                                    owner.digest(),
                                    &payload,
                                );

                                if !verification_result {
                                    return Err(StorageError::InvalidSignature);
                                }
                            }
                            _ => {
                                // Action metadata is not User, but existing is.
                                return Err(StorageError::InvalidSignature);
                            }
                        }
                    }
                    StorageType::Public => { /* No special checks */ }
                }
            }
            Action::Compare { .. } => { /* No checks needed */ }
        }

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
                debug!(
                    %id,
                    ancestor_ids = ?ancestors.iter().map(|a| a.id()).collect::<Vec<_>>(),
                    created_at = metadata.created_at,
                    updated_at = metadata.updated_at(),
                    data_len = data.len(),
                    "Interface::apply_action preparing to upsert entity"
                );
                let mut parent = None;
                for this in ancestors.iter().rev() {
                    let parent = parent.replace(this);

                    if <Index<S>>::has_index(this.id()) {
                        debug!(
                            ancestor = %this.id(),
                            "Ancestor already present in index - skipping creation"
                        );
                        continue;
                    }

                    let Some(parent) = parent else {
                        debug!(
                            ancestor = %this.id(),
                            "Creating ancestor as root index entry (no parent yet)"
                        );
                        <Index<S>>::add_root(this.clone())?;

                        continue;
                    };

                    // Set up parent-child relationship
                    debug!(
                        parent = %parent.id(),
                        child = %this.id(),
                        "Linking ancestor to parent in index"
                    );
                    <Index<S>>::add_child_to(parent.id(), this.clone())?;
                }

                // For new entities, create a minimal index entry first to avoid orphan errors
                if !<Index<S>>::has_index(id) {
                    if id.is_root() {
                        debug!(%id, "Creating root index entry for entity");
                        <Index<S>>::add_root(ChildInfo::new(id, [0; 32], metadata.clone()))?;
                    } else if let Some(parent) = parent {
                        // Create minimal index entry with placeholder hash
                        let placeholder_hash = Sha256::digest(&data).into();
                        debug!(
                            %id,
                            parent = %parent.id(),
                            placeholder_hash = ?placeholder_hash,
                            "Creating placeholder child entry pending save"
                        );
                        <Index<S>>::add_child_to(
                            parent.id(),
                            ChildInfo::new(id, placeholder_hash, metadata.clone()),
                        )?;
                    }
                }

                // Save data (might merge, producing different hash)
                let Some((_, _full_hash)) = Self::save_internal(id, &data, metadata.clone())?
                else {
                    debug!(
                        %id,
                        "Remote action produced no storage change (save_internal returned None)"
                    );
                    // we didn't save anything, so we skip updating the ancestors
                    return Ok(());
                };

                debug!(
                    %id,
                    ancestor_count = ancestors.len(),
                    "Applied Add/Update action to storage"
                );

                // ALWAYS update parent with correct hash after save (handles merging)
                // save_internal calls update_hash_for which updates child_index.own_hash
                if let Some(parent) = parent {
                    let (_, own_hash) =
                        <Index<S>>::get_hashes_for(id)?.ok_or(StorageError::IndexNotFound(id))?;

                    // Update parent relationship with the actual hash after any merging
                    debug!(
                        %id,
                        parent = %parent.id(),
                        own_hash = ?own_hash,
                        "Updating parent child info with final hash"
                    );
                    <Index<S>>::add_child_to(
                        parent.id(),
                        ChildInfo::new(id, own_hash, metadata.clone()),
                    )?;
                }

                crate::delta::push_action(Action::Compare { id });
                debug!(%id, "Queued compare action after apply");
            }
            Action::Compare { .. } => {
                return Err(StorageError::ActionNotAllowed("Compare".to_owned()))
            }
            Action::DeleteRef { id, deleted_at, .. } => {
                debug!(%id, deleted_at, "Applying DeleteRef action");
                Self::apply_delete_ref_action(id, deleted_at)?;
            }
        };

        debug!("Interface::apply_action completed");

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
            debug!(
                %id,
                deleted_at,
                "DeleteRef ignored because entity already tombstoned"
            );
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
            debug!(%id, deleted_at, "DeleteRef ignored because entity metadata missing");
            return Ok(());
        };

        // Guard: Local update is newer, deletion loses
        if deleted_at < *metadata.updated_at {
            // Local update wins, ignore older deletion
            debug!(
                %id,
                deleted_at,
                local_updated_at = metadata.updated_at(),
                "DeleteRef ignored because local update is newer"
            );
            return Ok(());
        }

        // Deletion wins - apply it
        let _ignored = S::storage_remove(Key::Entry(id));
        let _ignored = <Index<S>>::mark_deleted(id, deleted_at);
        debug!(
            %id,
            deleted_at,
            "DeleteRef applied - entity removed and tombstone updated"
        );

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
    pub fn children_of<D: Data>(parent_id: Id) -> Result<Vec<D>, StorageError> {
        let children_info = <Index<S>>::get_children_of(parent_id)?;
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
    pub fn child_info_for(parent_id: Id) -> Result<Vec<ChildInfo>, StorageError> {
        <Index<S>>::get_children_of(parent_id)
    }

    /// Merges two entity data blobs using CRDT semantics based on the metadata's crdt_type.
    ///
    /// # Returns
    /// - `Ok(Some(merged_bytes))` if merge succeeded - both sides should use this
    /// - `Ok(None)` if merge not applicable (e.g., Manual resolution needed)
    /// - `Err` if merge failed
    ///
    /// # CRDT Type Dispatch
    /// - **Built-in CRDTs** (LwwRegister, Counter, etc.): Merged in storage layer
    /// - **Custom types**: Try registered merge, fallback to LWW
    /// - **None** (legacy): Use LWW based on timestamps
    fn merge_by_crdt_type(
        local_data: &[u8],
        remote_data: &[u8],
        local_metadata: &Metadata,
        remote_metadata: &Metadata,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        Self::merge_by_crdt_type_with_callback(
            local_data,
            remote_data,
            local_metadata,
            remote_metadata,
            None,
        )
    }

    /// Merge entities with optional WASM callback for custom types.
    fn merge_by_crdt_type_with_callback(
        local_data: &[u8],
        remote_data: &[u8],
        local_metadata: &Metadata,
        remote_metadata: &Metadata,
        callback: Option<&dyn WasmMergeCallback>,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        #[allow(unused_imports)]
        use crate::collections::{LwwRegister, Mergeable};

        let crdt_type = local_metadata.crdt_type.as_ref();

        match crdt_type {
            // ════════════════════════════════════════════════════════
            // BUILT-IN CRDTs: Merge in storage layer (fast, no WASM)
            // ════════════════════════════════════════════════════════
            Some(CrdtType::LwwRegister) => {
                // LWW uses timestamps for deterministic resolution
                // Note: For typed LwwRegister<T>, the merge just compares timestamps
                // Here we're working with raw bytes, so compare metadata timestamps
                let winner = if remote_metadata.updated_at() >= local_metadata.updated_at() {
                    remote_data
                } else {
                    local_data
                };
                Ok(Some(winner.to_vec()))
            }

            Some(CrdtType::Counter) => {
                // Counter merges by summing per-node counts
                // Requires deserializing the Counter struct
                // For now, fallback to registry or LWW since Counter has complex internal structure
                Self::try_merge_via_registry_or_lww(
                    local_data,
                    remote_data,
                    local_metadata,
                    remote_metadata,
                )
            }

            Some(CrdtType::UnorderedMap)
            | Some(CrdtType::UnorderedSet)
            | Some(CrdtType::Vector) => {
                // Collections are merged at the entry level via their child IDs
                // The collection container itself uses LWW for its metadata
                let winner = if remote_metadata.updated_at() >= local_metadata.updated_at() {
                    remote_data
                } else {
                    local_data
                };
                Ok(Some(winner.to_vec()))
            }

            Some(CrdtType::Rga) => {
                // RGA is built on UnorderedMap, merge happens at character level
                let winner = if remote_metadata.updated_at() >= local_metadata.updated_at() {
                    remote_data
                } else {
                    local_data
                };
                Ok(Some(winner.to_vec()))
            }

            // ════════════════════════════════════════════════════════
            // CUSTOM TYPES: Use WASM callback, registry, or LWW fallback
            // ════════════════════════════════════════════════════════
            Some(CrdtType::Custom { type_name }) => {
                // Custom types need WASM callback for proper merge
                Self::try_merge_custom_with_registry(
                    type_name,
                    local_data,
                    remote_data,
                    local_metadata,
                    remote_metadata,
                    callback,
                )
            }

            // ════════════════════════════════════════════════════════
            // LEGACY: No type info, use LWW
            // ════════════════════════════════════════════════════════
            None => {
                // Legacy data - fallback to LWW
                let winner = if remote_metadata.updated_at() >= local_metadata.updated_at() {
                    remote_data
                } else {
                    local_data
                };
                Ok(Some(winner.to_vec()))
            }
        }
    }

    /// Try merge via registry, fallback to LWW if not registered.
    fn try_merge_via_registry_or_lww(
        local_data: &[u8],
        remote_data: &[u8],
        local_metadata: &Metadata,
        remote_metadata: &Metadata,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        // Try registered merge functions
        if let Some(result) = try_merge_registered(
            local_data,
            remote_data,
            local_metadata.updated_at(),
            remote_metadata.updated_at(),
        ) {
            match result {
                Ok(merged) => return Ok(Some(merged)),
                Err(_) => {} // Fall through to LWW
            }
        }

        // Fallback to LWW
        let winner = if remote_metadata.updated_at() >= local_metadata.updated_at() {
            remote_data
        } else {
            local_data
        };
        Ok(Some(winner.to_vec()))
    }

    /// Merge custom type using WASM callback, registry, or LWW fallback.
    ///
    /// Priority:
    /// 1. WASM callback (if provided) - for runtime-managed WASM merge
    /// 2. Type-name registry - for types registered via `register_crdt_merge`
    /// 3. Brute-force registry - legacy fallback
    /// 4. LWW fallback
    fn try_merge_custom_with_registry(
        type_name: &str,
        local_data: &[u8],
        remote_data: &[u8],
        local_metadata: &Metadata,
        remote_metadata: &Metadata,
        callback: Option<&dyn WasmMergeCallback>,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        // 1. Try WASM callback first (production path)
        if let Some(cb) = callback {
            match cb.merge_custom(
                type_name,
                local_data,
                remote_data,
                local_metadata.updated_at(),
                remote_metadata.updated_at(),
            ) {
                Ok(merged) => return Ok(Some(merged)),
                Err(e) => {
                    debug!("WASM merge failed for {}: {}, falling back", type_name, e);
                    // Fall through to registry/LWW
                }
            }
        }

        // 2. Try type-name registry (efficient lookup)
        if let Some(result) = try_merge_by_type_name(
            type_name,
            local_data,
            remote_data,
            local_metadata.updated_at(),
            remote_metadata.updated_at(),
        ) {
            match result {
                Ok(merged) => return Ok(Some(merged)),
                Err(e) => {
                    debug!(
                        "Type-name merge failed for {}: {}, falling back",
                        type_name, e
                    );
                    // Fall through to brute-force/LWW
                }
            }
        }

        // 3. Try brute-force registry (legacy fallback)
        if let Some(result) = try_merge_registered(
            local_data,
            remote_data,
            local_metadata.updated_at(),
            remote_metadata.updated_at(),
        ) {
            match result {
                Ok(merged) => return Ok(Some(merged)),
                Err(_) => {} // Fall through to LWW
            }
        }

        // 4. Fallback to LWW
        let winner = if remote_metadata.updated_at() >= local_metadata.updated_at() {
            remote_data
        } else {
            local_data
        };
        Ok(Some(winner.to_vec()))
    }

    /// Compares local and remote entity trees using CRDT-type-based merge.
    ///
    /// Compares Merkle hashes recursively, producing action lists for both sides.
    /// Returns `(local_actions, remote_actions)` to bring trees into sync.
    ///
    /// # CRDT Merge Behavior
    ///
    /// When own hashes differ (data conflict):
    /// - **Built-in CRDTs**: Merged using type-specific logic (LWW, sum, etc.)
    /// - **Custom types**: Uses registered merge function or falls back to LWW
    /// - **Legacy (None)**: Falls back to LWW
    ///
    /// The merged result is sent to BOTH sides to ensure convergence.
    ///
    /// For custom type merging via WASM, use `compare_trees_with_callback`.
    ///
    /// # Errors
    /// Returns error if index lookup or hash comparison fails.
    ///
    pub fn compare_trees(
        foreign_entity_data: Option<Vec<u8>>,
        foreign_index_data: ComparisonData,
    ) -> Result<(Vec<Action>, Vec<Action>), StorageError> {
        Self::compare_trees_with_callback(foreign_entity_data, foreign_index_data, None)
    }

    /// Compares trees with an optional WASM merge callback for custom types.
    ///
    /// This variant allows passing a callback for merging `CrdtType::Custom` types
    /// via WASM. Used by the runtime layer during state synchronization.
    ///
    /// # Arguments
    /// * `foreign_entity_data` - Optional serialized entity data from foreign node
    /// * `foreign_index_data` - Comparison metadata from foreign node
    /// * `merge_callback` - Optional callback for custom type merging via WASM
    ///
    /// # Errors
    /// Returns error if index lookup or hash comparison fails.
    pub fn compare_trees_with_callback(
        foreign_entity_data: Option<Vec<u8>>,
        foreign_index_data: ComparisonData,
        merge_callback: Option<&dyn WasmMergeCallback>,
    ) -> Result<(Vec<Action>, Vec<Action>), StorageError> {
        let mut actions: (Vec<Action>, Vec<Action>) = (vec![], vec![]);

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

        // Compare full Merkle hashes - if equal, trees are in sync
        if local_full_hash == foreign_index_data.full_hash {
            return Ok(actions);
        }

        // Compare own hashes - if different, need to merge the data
        if local_own_hash != foreign_index_data.own_hash {
            if let Some(foreign_entity_data) = foreign_entity_data {
                // Use CRDT-type-based merge dispatch (with optional WASM callback)
                match Self::merge_by_crdt_type_with_callback(
                    &local_entity,
                    &foreign_entity_data,
                    &local_metadata,
                    &foreign_index_data.metadata,
                    merge_callback,
                )? {
                    Some(merged_data) => {
                        // Determine which metadata to use (newer timestamp)
                        let (merged_metadata, merged_ancestors) =
                            if foreign_index_data.metadata.updated_at()
                                >= local_metadata.updated_at()
                            {
                                (
                                    foreign_index_data.metadata.clone(),
                                    foreign_index_data.ancestors.clone(),
                                )
                            } else {
                                (local_metadata.clone(), <Index<S>>::get_ancestors_of(id)?)
                            };

                        // Check if local needs update
                        if merged_data != local_entity {
                            actions.0.push(Action::Update {
                                id,
                                data: merged_data.clone(),
                                ancestors: merged_ancestors.clone(),
                                metadata: merged_metadata.clone(),
                            });
                        }

                        // Check if remote needs update
                        if merged_data != foreign_entity_data {
                            actions.1.push(Action::Update {
                                id,
                                data: merged_data,
                                ancestors: merged_ancestors,
                                metadata: merged_metadata,
                            });
                        }
                    }
                    None => {
                        // Manual resolution needed - both sides get Compare action
                        actions.0.push(Action::Compare { id });
                        actions.1.push(Action::Compare { id });
                    }
                }
            } else {
                // No foreign data but hashes differ - local wins by default
                actions.1.push(Action::Update {
                    id,
                    data: local_entity,
                    ancestors: <Index<S>>::get_ancestors_of(id)?,
                    metadata: local_metadata,
                });
            }
        }

        let local_collection_names = <Index<S>>::get_collection_names_for(id)?;

        let local_collections = local_collection_names
            .into_iter()
            .map(|name| {
                let children = <Index<S>>::get_children_of(id)?;
                Ok((name, children))
            })
            .collect::<Result<BTreeMap<_, _>, StorageError>>()?;

        // Compare children - check both local and foreign collections
        // First, handle collections that exist locally
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
                            // Child exists locally but not on foreign - send to foreign
                            if let Some(local_child) = Self::find_by_id_raw(*child_id) {
                                let metadata = <Index<S>>::get_metadata(*child_id)?
                                    .ok_or(StorageError::IndexNotFound(*child_id))?;

                                // FIX: Use child_id for ancestors, not parent id
                                actions.1.push(Action::Add {
                                    id: *child_id,
                                    data: local_child,
                                    ancestors: <Index<S>>::get_ancestors_of(*child_id)?,
                                    metadata,
                                });
                            }
                        }
                        // Hashes match, no action needed
                        _ => {}
                    }
                }

                // Children that exist in foreign but not locally
                for (child_id, _) in &foreign_child_map {
                    if !local_child_map.contains_key(child_id) {
                        // Foreign has a child we don't have - need to sync
                        actions.0.push(Action::Compare { id: *child_id });
                    }
                }
            }
        }

        // Check for foreign collections that don't exist locally
        for (foreign_coll_name, foreign_children) in &foreign_index_data.children {
            if !local_collections.contains_key(foreign_coll_name) {
                // Foreign has a collection we don't have at all
                // Need to request data for all children in this collection
                for child in foreign_children {
                    actions.0.push(Action::Compare { id: child.id() });
                }
            }
        }

        Ok(actions)
    }

    /// High-level method for complete tree synchronization.
    ///
    /// This method recursively compares trees and resolves all Compare actions
    /// by fetching data via the provided callback. It returns all actions needed
    /// to fully synchronize both sides, without any remaining Compare actions.
    ///
    /// The `get_foreign_data` callback is called for each Compare action to fetch
    /// the foreign entity's data and comparison metadata.
    ///
    /// # Errors
    /// Returns error if comparison, data fetching, or action application fails.
    ///
    pub fn sync_trees<F>(
        foreign_entity_data: Option<Vec<u8>>,
        foreign_index_data: ComparisonData,
        get_foreign_data: F,
    ) -> Result<(Vec<Action>, Vec<Action>), StorageError>
    where
        F: Fn(Id) -> Result<(Option<Vec<u8>>, ComparisonData), StorageError>,
    {
        const MAX_DEPTH: usize = 100;

        fn sync_recursive<S: StorageAdaptor, F>(
            foreign_entity_data: Option<Vec<u8>>,
            foreign_index_data: ComparisonData,
            get_foreign_data: &F,
            depth: usize,
        ) -> Result<(Vec<Action>, Vec<Action>), StorageError>
        where
            F: Fn(Id) -> Result<(Option<Vec<u8>>, ComparisonData), StorageError>,
        {
            if depth > MAX_DEPTH {
                return Err(StorageError::InvalidData(
                    "Maximum recursion depth exceeded in sync_trees".to_owned(),
                ));
            }

            let (mut local_actions, mut remote_actions) =
                Interface::<S>::compare_trees(foreign_entity_data, foreign_index_data)?;

            // Process Compare actions recursively
            let mut i = 0;
            while i < local_actions.len() {
                if let Action::Compare { id } = &local_actions[i] {
                    let child_id = *id;
                    // Remove the Compare action
                    local_actions.remove(i);

                    // Also remove corresponding Compare from remote if exists
                    if let Some(pos) = remote_actions
                        .iter()
                        .position(|a| matches!(a, Action::Compare { id } if *id == child_id))
                    {
                        remote_actions.remove(pos);
                    }

                    // Fetch foreign data and recurse
                    let (child_data, child_comparison) = get_foreign_data(child_id)?;
                    let (child_local, child_remote) = sync_recursive::<S, F>(
                        child_data,
                        child_comparison,
                        get_foreign_data,
                        depth + 1,
                    )?;

                    // Merge results
                    local_actions.extend(child_local);
                    remote_actions.extend(child_remote);
                } else {
                    i += 1;
                }
            }

            Ok((local_actions, remote_actions))
        }

        sync_recursive::<S, F>(
            foreign_entity_data,
            foreign_index_data,
            &get_foreign_data,
            0,
        )
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
                <Index<S>>::get_children_of(id).map(|children| (collection_name.clone(), children))
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
    pub fn has_children(parent_id: Id) -> Result<bool, StorageError> {
        <Index<S>>::has_children(parent_id)
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
    pub fn remove_child_from(parent_id: Id, child_id: Id) -> Result<bool, StorageError> {
        let child_exists = <Index<S>>::get_children_of(parent_id)?
            .iter()
            .any(|child| child.id() == child_id);
        if !child_exists {
            return Ok(false);
        }

        // This will act as our nonce
        let deleted_at = time_now();

        // Get metadata before removing index
        let mut metadata =
            <Index<S>>::get_metadata(child_id)?.ok_or(StorageError::IndexNotFound(child_id))?;

        // If this is a local user action, set the nonce
        if let StorageType::User { owner, .. } = metadata.storage_type {
            if *owner == crate::env::executor_id() {
                // Use the deletion timestamp as the nonce
                metadata.storage_type = StorageType::User {
                    owner,
                    signature_data: Some(SignatureData {
                        signature: [0; 64], // Placeholder, added by signer
                        nonce: deleted_at,
                    }),
                };
            }
        }

        <Index<S>>::remove_child_from(parent_id, child_id)?;

        // Use DeleteRef for efficient tombstone-based deletion.
        // More efficient than Delete: only sends ID + timestamp + metadata vs full ancestor tree.
        // The tombstone is created by remove_child_from, we just broadcast the deletion.
        crate::delta::push_action(Action::DeleteRef {
            id: child_id,
            deleted_at,
            // Pass the full metadata
            metadata,
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

        debug!(%id, has_root = root.is_some(), "commit_root invoked");
        let hash = if let Some(root) = root {
            if root.id() != id {
                return Err(StorageError::UnexpectedId(root.id()));
            }

            if !root.element().is_dirty() {
                return Ok(());
            }

            let data = to_vec(&root).map_err(|e| StorageError::SerializationError(e.into()))?;

            Self::save_raw(id, data, root.element().metadata.clone())?
        } else {
            <Index<S>>::get_hashes_for(id)?.map(|(full_hash, _)| full_hash)
        };

        if let Some(hash) = hash {
            crate::delta::commit_root(&hash)?;
        }

        debug!(%id, ?hash, "commit_root completed");
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

        let Some(hash) = Self::save_raw(entity.id(), data, entity.element().metadata.clone())?
        else {
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
        let _incoming_created_at = metadata.created_at;
        let _incoming_updated_at = metadata.updated_at();

        let last_metadata = <Index<S>>::get_metadata(id)?;
        let final_data = if let Some(last_metadata) = &last_metadata {
            // CRDT-based merge: root state ALWAYS merges, non-root uses LWW with merge fallback

            if id.is_root() {
                // Root entity (app state) - ALWAYS merge regardless of timestamp
                // This preserves CRDT semantics (Counter, UnorderedMap, etc.)
                // The root contains all application state; merging combines concurrent updates
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
            } else if last_metadata.updated_at > metadata.updated_at {
                // Non-root entity with older incoming timestamp - reject (LWW)
                return Ok(None);
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
                <Index<S>>::add_root(ChildInfo::new(id, [0_u8; 32], metadata.clone()))?;
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
        debug!(
            %id,
            data_len = data.len(),
            created_at = metadata.created_at,
            updated_at = metadata.updated_at(),
            "save_raw called"
        );
        if !id.is_root() && <Index<S>>::get_parent_id(id)?.is_none() {
            return Err(StorageError::CannotCreateOrphan(id));
        }

        let mut metadata = metadata.clone();
        // If this is a local user action, set the nonce
        if let StorageType::User {
            owner,
            signature_data,
        } = metadata.storage_type
        {
            if *owner == crate::env::executor_id() && signature_data.is_none() {
                // This is a new local action. Set the nonce.
                // Use the `updated_at` timestamp as the nonce.
                let nonce = *metadata.updated_at;
                metadata.storage_type = StorageType::User {
                    owner,
                    signature_data: Some(SignatureData {
                        signature: [0; 64], // Placeholder, added by signer
                        nonce,
                    }),
                };
            }
        }

        let Some((is_new, full_hash)) = Self::save_internal(id, &data, metadata.clone())? else {
            return Ok(None);
        };

        let ancestors = <Index<S>>::get_ancestors_of(id)?;

        let action = if is_new {
            debug!(%id, "save_raw emitting Add action for entity");
            Action::Add {
                id,
                data,
                ancestors,
                metadata,
            }
        } else {
            debug!(%id, "save_raw emitting Update action for entity");
            Action::Update {
                id,
                data,
                ancestors,
                metadata,
            }
        };

        crate::delta::push_action(action);

        debug!(%id, ?full_hash, is_new, "save_raw completed");

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

    /// Helper to verify a new `Update` action.
    fn verify_action_update(action: &Action) -> Result<(), StorageError> {
        let (metadata, _data, id) = match action {
            Action::Update {
                metadata, data, id, ..
            } => (metadata, data, *id),
            // Should not happen
            _ => return Ok(()),
        };

        // Get existing metadata
        let existing_metadata = <Index<S>>::get_metadata(id)?;

        // Try to get existing metadata to determine if this is an Update or an Add (upsert)
        match existing_metadata {
            // This is indeed an update operation
            Some(existing_metadata) => {
                // Compare storage types and owners
                match (&existing_metadata.storage_type, &metadata.storage_type) {
                    (StorageType::Public, StorageType::Public) => {
                        // no checks needed for Public storage
                        Ok(())
                    }
                    (StorageType::Frozen, StorageType::Frozen) => {
                        // Mutability is verified in the main `apply_action()` function later
                        Ok(())
                    }
                    (
                        StorageType::User {
                            owner: existing_owner,
                            ..
                        },
                        StorageType::User { owner, .. },
                    ) => {
                        // Check owner hasn't changed
                        if *owner != *existing_owner {
                            return Err(StorageError::ActionNotAllowed(
                                "Cannot change owner of User storage".to_owned(),
                            ));
                        }

                        Ok(())
                    }
                    (existing, new) => {
                        // All other combinations are invalid
                        crate::env::log(&format!(
                            "Invalid storage type change attempted: {:?} -> {:?}",
                            existing, new
                        ));
                        Err(StorageError::ActionNotAllowed(
                            "Cannot change StorageType (e.g., User->Public/User->Frozen/etc)"
                                .to_owned(),
                        ))
                    }
                }
            }
            None => {
                // This is an "add" (upsert).
                // TODO: refactor
                // The item doesn't exist. Run the "Add" verification logic (that is currently
                // located in the main `apply_function()`.
                Ok(())
            }
        }
    }
}

/// Verifies an incoming `Frozen` action.
fn verify_frozen_action_upsert(action: &Action, data: &[u8]) -> Result<(), StorageError> {
    // Block all Updates.
    if let Action::Update { .. } = action {
        return Err(StorageError::ActionNotAllowed(
            "Frozen data cannot be updated".to_owned(),
        ));
    }

    // Verify the content-addressing via byte-slicing.
    // The data blob is: [key_hash (32 bytes)] + [value_bytes (N bytes)] + [element_id (32 bytes)]
    const KEY_HASH_SIZE: usize = 32;
    const ELEMENT_ID_SIZE: usize = 32;
    const MIN_LEN: usize = KEY_HASH_SIZE + ELEMENT_ID_SIZE;

    if data.len() < MIN_LEN {
        return Err(StorageError::InvalidData(
            "Frozen data blob is too small.".to_owned(),
        ));
    }

    // Extract the three components
    let key_from_entry = &data[..KEY_HASH_SIZE];
    // We don't need the `Element::Id` from the end, but we know it's there and
    // we need to remove it from the value_bytes.
    let value_bytes = &data[KEY_HASH_SIZE..data.len() - ELEMENT_ID_SIZE];

    // Re-calculate the hash of the `value bytes`
    let calculated_hash: [u8; 32] = Sha256::digest(value_bytes).into();

    // Check: The key inside the `Entry` must match the hash
    // of the value inside the `Entry`.
    if key_from_entry != calculated_hash {
        return Err(StorageError::InvalidData(
            "Frozen data corruption: Entry key does not match hash of Entry value.".to_owned(),
        ));
    }

    // If this check passes, the data is verified.
    Ok(())
}
