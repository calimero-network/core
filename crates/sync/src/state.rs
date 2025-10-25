//! Synchronization state tracking for remote nodes.

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_storage::address::Id;
use calimero_storage::env::time_now;
use calimero_storage::error::StorageError;
use calimero_storage::store::{Key, StorageAdaptor};

/// Tracks synchronization state with a remote node.
///
/// Used to determine when full resync is needed vs incremental sync.
/// Persisted per remote node ID in storage under Key::SyncState.
///
#[derive(Copy, Clone, Debug, BorshDeserialize, BorshSerialize, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
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
    /// Creates a new sync state for a remote node.
    #[must_use]
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
    /// Returns true if the time since last sync exceeds the tombstone retention period.
    /// When true, incremental sync may fail due to missing tombstones, requiring full resync.
    #[must_use]
    pub fn needs_full_resync(&self, tombstone_retention_nanos: u64) -> bool {
        let now = time_now();
        let offline_duration = now.saturating_sub(self.last_sync_time);
        offline_duration > tombstone_retention_nanos
    }

    /// Updates sync state after successful sync.
    pub fn update(&mut self, root_hash: [u8; 32]) {
        self.last_sync_time = time_now();
        self.last_sync_root_hash = root_hash;
        self.sync_count += 1;
    }
}

/// Retrieves sync state for a remote node.
///
/// # Errors
/// Returns error if deserialization fails.
pub fn get_sync_state<S: StorageAdaptor>(node_id: Id) -> Result<Option<SyncState>, StorageError> {
    let Some(data) = S::storage_read(Key::SyncState(node_id)) else {
        return Ok(None);
    };

    let state = borsh::from_slice(&data).map_err(StorageError::DeserializationError)?;
    Ok(Some(state))
}

/// Saves sync state for a remote node.
///
/// # Errors
/// Returns error if serialization fails.
pub fn save_sync_state<S: StorageAdaptor>(state: &SyncState) -> Result<(), StorageError> {
    let data = borsh::to_vec(state).map_err(|e| StorageError::SerializationError(e.into()))?;
    S::storage_write(Key::SyncState(state.node_id), &data);
    Ok(())
}

