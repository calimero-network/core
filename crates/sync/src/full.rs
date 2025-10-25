//! Full resync protocol using complete storage snapshots.
//!
//! This module handles complete state transfer when delta synchronization is
//! not possible due to tombstone expiration (nodes offline > retention period).
//!
//! ## Protocol
//!
//! Uses `Snapshot` from `calimero-storage` - a complete dump of all storage state.
//!
//! ```text
//! Node A (offline > 2 days)               Node B (current)
//! ┌─────────────────┐                     ┌─────────────────┐
//! │  Old State      │                     │  Current State  │
//! │                 │                     │       ↓         │
//! │  1. Request ────┼──── needs sync ────→│ generate_snapshot()
//! │                 │                     │       ↓         │
//! │                 │◀──── Snapshot ──────┤  2. Send        │
//! │  3. Clear all   │   (full dump)       │                 │
//! │  4. Apply       │                     │                 │
//! │       ↓         │                     │                 │
//! │  Current State  │                     │  Current State  │
//! └─────────────────┘                     └─────────────────┘
//! ```

use calimero_storage::address::Id;
use calimero_storage::error::StorageError;
use calimero_storage::snapshot::{apply_snapshot, Snapshot};
use calimero_storage::store::IterableStorage;

use crate::state::{get_sync_state, save_sync_state, SyncState};

// Snapshot is now re-exported from calimero-storage::snapshot
// (already done in lib.rs)

/// Performs full resync protocol with a remote node.
///
/// This is the high-level orchestration function that:
/// 1. Validates the snapshot from the remote node
/// 2. Applies it to local storage (clearing existing state)
/// 3. Updates sync state tracking
///
/// # Safety
///
/// This function **deletes all local storage data**.
/// Only call during controlled resync operations after confirming
/// the snapshot is from a trusted peer.
///
/// # Arguments
///
/// * `remote_node_id` - ID of the node providing the snapshot
/// * `snapshot` - Complete storage dump from the remote node
///
/// # Errors
///
/// - `InvalidData` if snapshot is empty
/// - Storage errors if read/write operations fail
///
pub fn full_resync<S: IterableStorage>(
    remote_node_id: Id,
    snapshot: Snapshot,
) -> Result<(), StorageError> {
    // Step 1: Validate snapshot
    if snapshot.entity_count == 0 && snapshot.index_count == 0 {
        return Err(StorageError::InvalidData(
            "Snapshot is empty".to_owned(),
        ));
    }

    // Step 2: Apply snapshot using storage function
    apply_snapshot::<S>(&snapshot)?;

    // Step 3: Update sync state
    let mut sync_state = get_sync_state::<S>(remote_node_id)?
        .unwrap_or_else(|| SyncState::new(remote_node_id));

    sync_state.update([0; 32]); // TODO: Use actual root hash from snapshot
    save_sync_state::<S>(&sync_state)?;

    Ok(())
}

// Re-export snapshot generation from storage for convenience
pub use calimero_storage::snapshot::generate_snapshot;

// NOTE: Network layer orchestration
//
// The protocol flow for full resync:
// 1. Node A checks if it needs full resync (SyncState::needs_full_resync)
// 2. Node A requests snapshot from Node B
// 3. Node B calls generate_snapshot() and sends result
// 4. Node A validates and calls full_resync(remote_id, snapshot)
//
// Example network layer implementation:
//
// ```rust
// async fn handle_snapshot_request(peer_id: NodeId) -> Result<()> {
//     let snapshot = generate_snapshot::<MainStorage>()?;
//     network::send_snapshot(peer_id, snapshot).await
// }
// ```
