//! CRDT synchronization for Calimero distributed storage.
//!
//! This crate provides high-level state management and synchronization
//! built on top of `calimero-storage` primitives.
//!
//! # Three Synchronization Strategies
//!
//! 1. **Live Sync** (`live`) - Real-time action broadcasting
//!    - Immediate propagation during WASM execution
//!    - Lowest latency
//!    - Used for active operations
//!
//! 2. **Delta Sync** (`delta`) - Partial state reconciliation
//!    - Merkle tree comparison
//!    - Efficient for small divergences
//!    - Used for catching up on recent changes
//!
//! 3. **Full Sync** (`full`) - Complete state transfer
//!    - Snapshot-based
//!    - Used when offline > tombstone retention (>2 days)
//!    - Handles split-brain scenarios
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │     calimero-sync (this crate)      │
//! │   - Interface (CRDT operations)     │
//! │   - Actions (Add, Update, Delete)   │
//! │   - Sync strategies (live/delta/full)│
//! └─────────────────────────────────────┘
//!              ↓ uses
//! ┌─────────────────────────────────────┐
//! │      calimero-storage               │
//! │   - Storage primitives (keys, etc)  │
//! │   - Index management                │
//! │   - Low-level adapters              │
//! └─────────────────────────────────────┘
//! ```

#![forbid(unreachable_pub, unsafe_op_in_unsafe_fn)]
#![deny(
    unsafe_code,
    clippy::expect_used,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_in_result,
    clippy::unwrap_used
)]
#![warn(
    missing_docs,
    clippy::future_not_send,
    clippy::let_underscore_untyped,
    clippy::map_err_ignore,
    clippy::pattern_type_mismatch,
    clippy::same_name_method,
    clippy::shadow_reuse,
    clippy::shadow_same,
    clippy::shadow_unrelated,
    clippy::unreachable,
    clippy::use_debug
)]

pub mod delta;
pub mod full;
pub mod live;
pub mod manager;
pub mod state;

// Network protocol layer
#[cfg(not(target_arch = "wasm32"))]
pub mod network;

#[cfg(test)]
mod tests;

// Re-export commonly used types
pub use delta::DeltaSync;
pub use live::LiveSync;
pub use manager::{SyncManager, SyncStrategy};
pub use state::{get_sync_state, save_sync_state, SyncState};

// Re-export network layer (node-level)
#[cfg(not(target_arch = "wasm32"))]
pub use network::{NetworkSyncManager, SyncConfig};

// Re-export storage primitives for convenience
pub use calimero_storage::action::Action;
pub use calimero_storage::delta::{commit_root, push_action, push_comparison, StorageDelta};
pub use calimero_storage::error::StorageError;
pub use calimero_storage::snapshot::Snapshot;

