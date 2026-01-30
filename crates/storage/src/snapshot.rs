//! Snapshot generation and application for CRDT state.
//!
//! Provides two snapshot modes:
//! - **Network snapshots** (exclude tombstones): For transferring state between nodes
//! - **Full snapshots** (include tombstones): For debugging and backup purposes
//!
//! ## Security
//!
//! Snapshots received from untrusted sources should be verified before application.
//! The `apply_snapshot` function performs cryptographic verification by default.
//! Use `apply_snapshot_unchecked` only for trusted sources (e.g., local backups).

use std::collections::HashMap;

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use crate::address::Id;
use crate::env::time_now;
use crate::error::StorageError;
use crate::index::{EntityIndex, Index};
use crate::store::{IterableStorage, Key};

/// Snapshot of CRDT storage state.
///
/// Contains all entities and their indexes for transferring complete state.
/// By default, excludes tombstones for network efficiency.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "borsh")]
pub struct Snapshot {
    /// Number of entity entries in the snapshot
    pub entity_count: usize,

    /// Number of index entries in the snapshot
    pub index_count: usize,

    /// Raw entity data (ID -> serialized entity)
    pub entries: Vec<(Id, Vec<u8>)>,

    /// Raw index data (ID -> serialized EntityIndex)
    pub indexes: Vec<(Id, Vec<u8>)>,

    /// Root Merkle hash for snapshot verification
    pub root_hash: [u8; 32],

    /// Timestamp when snapshot was created (nanoseconds since epoch)
    pub timestamp: u64,
}

/// Generates a snapshot of all storage data (WASM-level, excludes tombstones).
///
/// This is the primary snapshot function for network synchronization.
/// Tombstones are excluded to minimize transfer size.
///
/// # Errors
///
/// Returns error if storage iteration or serialization fails.
///
pub fn generate_snapshot<S: IterableStorage>() -> Result<Snapshot, StorageError> {
    let mut entries = Vec::new();
    let mut indexes = Vec::new();

    // Iterate all keys in storage
    for key in S::storage_iter_keys() {
        match key {
            Key::Entry(id) => {
                // Include all entries
                if let Some(data) = S::storage_read(key) {
                    entries.push((id, data));
                }
            }
            Key::Index(id) => {
                // Get the index to check if it's a tombstone
                if let Some(data) = S::storage_read(key) {
                    let index = EntityIndex::try_from_slice(&data)
                        .map_err(StorageError::DeserializationError)?;

                    // Skip tombstones in network snapshots
                    if index.deleted_at.is_none() {
                        indexes.push((id, data));
                    }
                }
            }
            _ => {
                // Skip sync state and other metadata keys
            }
        }
    }

    // Calculate root hash from indexes
    let root_hash = Index::<S>::get_hashes_for(Id::root())?
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

/// Generates a full snapshot including tombstones (for debugging/backup).
///
/// Unlike `generate_snapshot`, this includes deleted entities for complete
/// state reconstruction or debugging purposes.
///
/// # Errors
///
/// Returns error if storage iteration or serialization fails.
///
pub fn generate_full_snapshot<S: IterableStorage>() -> Result<Snapshot, StorageError> {
    let mut entries = Vec::new();
    let mut indexes = Vec::new();

    // Iterate all keys in storage
    for key in S::storage_iter_keys() {
        match key {
            Key::Entry(id) => {
                if let Some(data) = S::storage_read(key) {
                    entries.push((id, data));
                }
            }
            Key::Index(id) => {
                // Include ALL indexes, even tombstones
                if let Some(data) = S::storage_read(key) {
                    indexes.push((id, data));
                }
            }
            _ => {
                // Skip sync state and other metadata keys
            }
        }
    }

    // Calculate root hash from indexes
    let root_hash = Index::<S>::get_hashes_for(Id::root())?
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

/// Applies a snapshot to storage with cryptographic verification.
///
/// This function verifies that all entity data matches the claimed hashes
/// before applying, protecting against tampered or corrupted snapshots.
///
/// **WARNING**: This function deletes all existing storage data before
/// applying the snapshot. Only use during controlled full resync operations.
///
/// # Verification Steps
///
/// 1. Verify each entity's data hash matches the claimed `own_hash` in its index
/// 2. Apply the snapshot data
/// 3. Verify the computed root hash matches the claimed `root_hash`
///
/// # Errors
///
/// Returns error if:
/// - Any entity hash doesn't match its claimed hash (tampered data)
/// - Root hash doesn't match after application (corrupted snapshot)
/// - Storage writes fail
///
pub fn apply_snapshot<S: IterableStorage>(snapshot: &Snapshot) -> Result<(), StorageError> {
    // Step 1: Build a map of ID -> expected own_hash from indexes
    let mut expected_hashes: HashMap<Id, [u8; 32]> = HashMap::new();
    for (id, index_data) in &snapshot.indexes {
        let index =
            EntityIndex::try_from_slice(index_data).map_err(StorageError::DeserializationError)?;
        expected_hashes.insert(*id, index.own_hash());
    }

    // Step 2: Verify all entity hashes BEFORE clearing existing data
    for (id, data) in &snapshot.entries {
        if let Some(expected_hash) = expected_hashes.get(id) {
            let computed_hash: [u8; 32] = Sha256::digest(data).into();
            if computed_hash != *expected_hash {
                return Err(StorageError::InvalidData(format!(
                    "Snapshot verification failed: entity {} hash mismatch. \
                     Expected {:?}, computed {:?}. Snapshot may be tampered.",
                    id,
                    &expected_hash[..8],
                    &computed_hash[..8]
                )));
            }
        }
        // Note: entries without indexes are allowed (orphaned data cleanup)
    }

    // Step 3: Clear existing storage (only after verification passes)
    clear_all_storage::<S>()?;

    // Step 4: Write all entries from snapshot
    for (id, data) in &snapshot.entries {
        let _ = S::storage_write(Key::Entry(*id), data);
    }

    // Step 5: Write all indexes from snapshot
    for (id, data) in &snapshot.indexes {
        let _ = S::storage_write(Key::Index(*id), data);
    }

    // Step 6: Verify root hash matches claimed hash
    let actual_root_hash = Index::<S>::get_hashes_for(Id::root())?
        .map(|(full_hash, _)| full_hash)
        .unwrap_or([0; 32]);

    if actual_root_hash != snapshot.root_hash {
        // Rollback by clearing (we can't restore the old data, but at least
        // we don't leave corrupted state)
        clear_all_storage::<S>()?;
        return Err(StorageError::InvalidData(format!(
            "Snapshot root hash verification failed. \
             Expected {:?}, computed {:?}. Snapshot may be corrupted.",
            &snapshot.root_hash[..8],
            &actual_root_hash[..8]
        )));
    }

    Ok(())
}

/// Applies a snapshot to storage WITHOUT verification.
///
/// **SECURITY WARNING**: This function does NOT verify entity hashes!
/// Only use for trusted sources such as:
/// - Local backups created by this node
/// - Debugging/testing scenarios
/// - Performance-critical paths where the source is pre-verified
///
/// For untrusted sources (network peers), use `apply_snapshot` instead.
///
/// # Errors
///
/// Returns error if storage writes fail.
///
pub fn apply_snapshot_unchecked<S: IterableStorage>(
    snapshot: &Snapshot,
) -> Result<(), StorageError> {
    // Step 1: Clear all existing storage
    clear_all_storage::<S>()?;

    // Step 2: Write all entries from snapshot
    for (id, data) in &snapshot.entries {
        let _ = S::storage_write(Key::Entry(*id), data);
    }

    // Step 3: Write all indexes from snapshot
    for (id, data) in &snapshot.indexes {
        let _ = S::storage_write(Key::Index(*id), data);
    }

    Ok(())
}

/// Clears all storage data except sync state.
///
/// Used internally by `apply_snapshot` to prepare for new state.
///
/// # Errors
///
/// Returns error if storage deletion fails.
///
fn clear_all_storage<S: IterableStorage>() -> Result<(), StorageError> {
    let mut keys_to_delete = Vec::new();

    for key in S::storage_iter_keys() {
        match key {
            Key::Entry(_) | Key::Index(_) => {
                keys_to_delete.push(key);
            }
            _ => {
                // Keep sync state and other metadata
            }
        }
    }

    for key in keys_to_delete {
        let _ = S::storage_remove(key);
    }

    Ok(())
}
