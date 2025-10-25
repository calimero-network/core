//! Network-level synchronization protocols.
//!
//! This module provides the network protocol layer for synchronization,
//! handling stream communication, encryption, and message serialization.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────┐
//! │  Node                            │
//! │  - Starts NetworkSyncManager     │
//! │  - Provides network/context clients │
//! └────────────┬─────────────────────┘
//!              │
//!              ├─ Uses
//!              │
//! ┌────────────▼─────────────────────┐
//! │  calimero-sync/network           │
//! │  - NetworkSyncManager            │
//! │  - Protocol handlers             │
//! │    • full.rs (snapshot transfer) │
//! │    • delta.rs (merkle sync)      │
//! │    • state.rs (legacy)           │
//! │    • key.rs (key sharing)        │
//! │    • blobs.rs (blob sharing)     │
//! └────────────┬─────────────────────┘
//!              │
//!              ├─ Uses
//!              │
//! ┌────────────▼─────────────────────┐
//! │  calimero-sync (strategy layer)  │
//! │  - SyncManager (strategy)        │
//! │  - Snapshot generation           │
//! │  - SyncState tracking            │
//! └──────────────────────────────────┘
//! ```

mod blobs;
mod delta;
mod full;
mod key;
mod manager;
mod state;

pub use manager::{NetworkSyncManager, SyncConfig};

