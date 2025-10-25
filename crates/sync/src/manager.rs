//! Sync manager - orchestrates all synchronization strategies.
//!
//! This is the main entry point for synchronization. It uses `SyncState` to
//! determine which sync strategy to use for a given peer.
//!
//! ## Decision Tree
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │  sync_with_peer(peer_id)                    │
//! └─────────────┬───────────────────────────────┘
//!               │
//!               ├─ Check SyncState
//!               │
//!   ┌───────────▼──────────┐
//!   │ Never synced OR      │
//!   │ Offline > retention? │──YES──→ Full Resync
//!   └───────────┬──────────┘         (snapshot transfer)
//!               │
//!              NO
//!               │
//!   ┌───────────▼──────────┐
//!   │ Recently synced?     │
//!   │ Root hashes differ?  │──YES──→ Delta Sync
//!   └───────────┬──────────┘         (Merkle comparison)
//!               │
//!              NO
//!               │
//!   ┌───────────▼──────────┐
//!   │ Already in sync      │
//!   │ Active connection?   │──YES──→ Live Sync
//!   └──────────────────────┘         (action broadcast)
//! ```

use calimero_storage::address::Id;
use calimero_storage::constants::TOMBSTONE_RETENTION_NANOS;
use calimero_storage::error::StorageError;
use calimero_storage::store::IterableStorage;

use crate::state::{get_sync_state, SyncState};

/// Synchronization manager coordinating all sync strategies.
///
/// This is the main API for node-to-node synchronization. It automatically
/// selects the appropriate sync strategy based on sync history and state.
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sync::SyncManager;
/// use calimero_storage::address::Id;
/// use calimero_storage::env::MainStorage;
///
/// let manager = SyncManager::<MainStorage>::new();
/// let peer_id = Id::new([1; 32]);
///
/// // Manager automatically selects full/delta/live sync
/// // manager.sync_with_peer(peer_id)?;
/// ```
#[derive(Debug)]
pub struct SyncManager<S: IterableStorage> {
    _phantom: std::marker::PhantomData<S>,
}

impl<S: IterableStorage> SyncManager<S> {
    /// Creates a new sync manager.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    /// Determines which sync strategy to use for a peer.
    ///
    /// Decision logic:
    /// - **Full resync** if never synced or offline > tombstone retention
    /// - **Delta sync** if recently synced but state diverged
    /// - **Live sync** if actively connected and in sync
    ///
    /// # Errors
    ///
    /// Returns error if storage operations fail.
    ///
    pub fn determine_sync_strategy(
        &self,
        peer_id: Id,
    ) -> Result<SyncStrategy, StorageError> {
        // Check if we need full resync based on sync state
        if let Some(state) = get_sync_state::<S>(peer_id)? {
            if state.needs_full_resync(TOMBSTONE_RETENTION_NANOS) {
                return Ok(SyncStrategy::Full);
            }
        } else {
            // Never synced before → full resync
            return Ok(SyncStrategy::Full);
        }

        // Get sync state to determine next steps
        let sync_state = get_sync_state::<S>(peer_id)?;

        match sync_state {
            None => {
                // Never synced before → full resync
                Ok(SyncStrategy::Full)
            }
            Some(_state) => {
                // Recently synced → use delta sync to catch up
                // TODO: Add root hash comparison here to detect if delta sync is needed
                // For now, default to delta sync for all recently-synced peers
                Ok(SyncStrategy::Delta)
            }
        }
    }

    /// Synchronizes with a peer using the appropriate strategy.
    ///
    /// This is the main entry point for synchronization. It:
    /// 1. Determines the appropriate sync strategy
    /// 2. Executes the sync
    /// 3. Updates sync state
    ///
    /// # Arguments
    ///
    /// * `peer_id` - ID of the peer to sync with
    ///
    /// # Errors
    ///
    /// Returns error if sync fails or storage operations fail.
    ///
    /// # TODO
    ///
    /// - [ ] Add network layer integration
    /// - [ ] Add progress reporting
    /// - [ ] Add error recovery and retries
    /// - [ ] Add rate limiting
    /// - [ ] Add metrics (sync duration, bytes transferred, etc.)
    ///
    pub fn sync_with_peer(&self, peer_id: Id) -> Result<SyncStrategy, StorageError> {
        let strategy = self.determine_sync_strategy(peer_id)?;

        // TODO: Actually execute the sync based on strategy
        // For now, just return what strategy would be used
        //
        // match strategy {
        //     SyncStrategy::Full => {
        //         // 1. Request snapshot from peer
        //         // 2. Apply using full::full_resync()
        //     }
        //     SyncStrategy::Delta => {
        //         // 1. Exchange Merkle tree comparisons
        //         // 2. Sync differences
        //     }
        //     SyncStrategy::Live => {
        //         // 1. Subscribe to peer's action stream
        //         // 2. Broadcast our actions to peer
        //     }
        // }

        Ok(strategy)
    }

    /// Starts live sync with a peer (active connection).
    ///
    /// Use this when establishing a real-time connection with a peer
    /// for continuous action broadcasting.
    ///
    /// # Errors
    ///
    /// Returns error if connection setup fails.
    ///
    /// # TODO
    ///
    /// - [ ] Implement action stream subscription
    /// - [ ] Add connection lifecycle management
    ///
    pub fn start_live_sync(&self, _peer_id: Id) -> Result<(), StorageError> {
        // TODO: Set up bidirectional action streaming
        Ok(())
    }

    /// Stops live sync with a peer.
    ///
    /// # Errors
    ///
    /// Returns error if cleanup fails.
    ///
    pub fn stop_live_sync(&self, _peer_id: Id) -> Result<(), StorageError> {
        // TODO: Tear down action streams
        Ok(())
    }
}

impl<S: IterableStorage> Default for SyncManager<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// Synchronization strategy selected for a peer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SyncStrategy {
    /// Full state transfer via snapshot (peer offline > retention period).
    Full,
    
    /// Merkle tree comparison and delta sync (peer recently offline).
    Delta,
    
    /// Real-time action broadcasting (peer actively connected).
    Live,
}

impl std::fmt::Display for SyncStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "full resync"),
            Self::Delta => write!(f, "delta sync"),
            Self::Live => write!(f, "live sync"),
        }
    }
}

