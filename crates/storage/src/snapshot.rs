//! Snapshot generation and application for CRDT state.
//!
//! Provides two snapshot modes:
//! - **Network snapshots** (exclude tombstones): For transferring state between nodes
//! - **Full snapshots** (include tombstones): For debugging and backup purposes

use borsh::{BorshDeserialize, BorshSerialize};

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

    /// Raw child-trie rows (addressed ID -> serialized trie node/bucket).
    ///
    /// A parent's children live in `Key::ChildTrie`, not inside its index row.
    /// Shipping only entries and indexes therefore installs every entity with
    /// no parent->child link at all: `get_children_of` reads empty and every
    /// parent's stored `full_hash`, which folds the trie root, stops matching a
    /// recompute. The node-side sync path rebuilds these links explicitly after
    /// installing pages; this module has no such pass, so it has to carry them.
    pub trie_rows: Vec<(Id, Vec<u8>)>,

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
    let mut trie_rows = Vec::new();

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
            Key::ChildTrie(id) => {
                if let Some(data) = S::storage_read(key) {
                    trie_rows.push((id, data));
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
        trie_rows,
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
    let mut trie_rows = Vec::new();

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
            Key::ChildTrie(id) => {
                if let Some(data) = S::storage_read(key) {
                    trie_rows.push((id, data));
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
        trie_rows,
        root_hash,
        timestamp: time_now(),
    })
}

/// Applies a snapshot to storage, replacing all existing data.
///
/// **WARNING**: This function deletes all existing storage data before
/// applying the snapshot. Only use during controlled full resync operations.
///
/// # Errors
///
/// Returns error if storage writes fail.
///
pub fn apply_snapshot<S: IterableStorage>(snapshot: &Snapshot) -> Result<(), StorageError> {
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

    // Step 4: Restore the child tries. Without this every entity lands with no
    // link to its parent, which is unrecoverable in place: re-applying
    // byte-identical entities takes the update path, never `add_child_to`, so
    // nothing ever re-links them.
    for (id, data) in &snapshot.trie_rows {
        let _ = S::storage_write(Key::ChildTrie(*id), data);
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
            Key::Entry(_) | Key::Index(_) | Key::ChildTrie(_) => {
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
