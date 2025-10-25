//! Live state mutation broadcasting.
//!
//! Broadcasts `StateMutation` events in real-time as they occur after WASM execution.
//! This is the lowest-latency sync method, propagating changes immediately via gossipsub.
//!
//! ## What Gets Broadcast
//!
//! **`StateMutation`** - Complete state change after WASM execution:
//! - New root hash
//! - Serialized `StorageDelta` (batched actions)
//! - Application output
//! - Broadcast to all context members via gossipsub
//!
//! ## Architecture
//!
//! ```text
//! Node A (WASM completes)                  Node B (subscribed)
//! ┌──────────────┐                        ┌──────────────┐
//! │ WASM finishes│                        │              │
//! │   ↓          │                        │              │
//! │ commit_root()│                        │              │
//! │   ↓          │                        │              │
//! │ StateMutation│ ───── gossipsub ────→  │ Receive event│
//! │ (with delta) │                        │   ↓          │
//! │              │                        │ Execute WASM │
//! │              │                        │  (sync call) │
//! └──────────────┘                        └──────────────┘
//! ```
//!
//! ## Protocol Flow
//!
//! 1. Node A executes WASM method
//! 2. WASM collects actions via `push_action()` during execution
//! 3. At `commit_root()`, actions are batched into `StorageDelta`
//! 4. Node broadcasts `StateMutation` event to gossipsub topic
//! 5. Node B receives event and executes `__calimero_sync_next` with delta
//! 6. Both nodes converge to same root hash
//!
//! ## Live vs Delta vs Full
//!
//! - **Live** (this module): Gossipsub broadcast of StateMutation after every execution
//!   - Real-time propagation
//!   - All connected peers get updates immediately
//!   - Used for active context participation
//!
//! - **Delta**: Merkle tree comparison on-demand
//!   - Used for catching up after brief offline
//!   - P2P streams, not gossipsub
//!
//! - **Full**: Snapshot transfer
//!   - Used for long-offline nodes (> 2 days)
//!   - P2P streams, not gossipsub

/// Live action synchronization manager.
///
/// Handles real-time broadcasting of CRDT actions to connected peers.
/// Provides the lowest-latency sync by propagating changes immediately.
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sync::LiveSync;
/// use calimero_storage::action::Action;
/// use calimero_storage::address::Id;
///
/// let live_sync = LiveSync::new();
///
/// // During WASM execution, broadcast actions:
/// let action = Action::Compare { id: Id::new([1; 32]) };
/// // live_sync.broadcast(action)?;
/// ```
#[derive(Debug)]
pub struct LiveSync {
    // TODO: Add fields for:
    // - peer connections
    // - action queue
    // - broadcast channel
}

impl LiveSync {
    /// Creates a new live sync manager.
    #[must_use]
    pub const fn new() -> Self {
        Self {}
    }

    // TODO: Implement methods:
    // - broadcast(action: Action) -> Result<()>
    // - handle_incoming(action: Action) -> Result<()>
    // - subscribe() -> ActionStream
    // - metrics
}

impl Default for LiveSync {
    fn default() -> Self {
        Self::new()
    }
}

