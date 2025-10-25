//! Peer synchronization protocols and coordination.
//!
//! This module handles all aspects of state synchronization between nodes:
//! - Network protocols (libp2p streams, encryption)
//! - Sync strategy decisions (full vs delta)
//! - Peer state tracking
//! - Protocol implementations (full, delta, state)
//! - Ancillary protocols (key sharing, blob sharing)
//!
//! ## Architecture (SOLID Principles Applied)
//!
//! ```text
//! SyncManager
//! ├── Orchestrates: periodic sync, peer selection
//! ├── Decides: Use delta or full resync
//! ├── Delegates to:
//! │   ├── full.rs    - Snapshot transfer protocol
//! │   ├── delta.rs   - Merkle comparison protocol  
//! │   ├── state.rs   - Legacy state sync
//! │   ├── key.rs     - Key sharing
//! │   └── blobs.rs   - Blob sharing
//! └── Tracks: peer_state.rs (per-peer sync history)
//! ```
//!
//! ## Principles Applied
//!
//! - **SRP**: Each protocol file has ONE job (full/delta/key/blobs)
//! - **KISS**: Simple decision: try delta → fallback to full
//! - **YAGNI**: No over-abstraction, no unnecessary crates
//! - **DRY**: Shared utilities (Sequencer, send/recv) in manager

mod blobs;
mod delta;
mod full;
mod key;
mod manager;
mod state;

pub use manager::{SyncConfig, SyncManager};
