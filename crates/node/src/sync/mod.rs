//! Peer synchronization protocols and coordination.
//!
//! This module handles all aspects of state synchronization between nodes:
//! - Network protocols (libp2p streams, encryption)
//! - Sync strategy decisions (full vs delta)
//! - Peer state tracking
//! - Protocol implementations (full, delta, state)
//! - Ancillary protocols (key sharing, blob sharing)
//! - Metrics and observability
//!
//! ## Architecture (SOLID Principles Applied)
//!
//! ```text
//! SyncManager
//! ├── Orchestrates: periodic sync, peer selection
//! ├── Decides: Use delta or full resync
//! ├── Delegates to:
//! │   ├── hash_comparison_protocol.rs - Merkle tree traversal (DFS)
//! │   ├── level_sync.rs               - Level-wise sync (BFS for wide trees)
//! │   ├── snapshot.rs                 - Snapshot transfer protocol
//! │   ├── key.rs                      - Key sharing
//! │   └── blobs.rs                    - Blob sharing
//! ├── Tracks: tracking.rs (per-peer sync history)
//! └── Observes: metrics.rs (protocol cost, safety invariants)
//! ```
//!
//! ## Metrics
//!
//! The sync module provides unified metrics through `SyncMetricsCollector`:
//! - Protocol cost: messages, bytes, round trips, entities, merges
//! - Phase timing: handshake, data_transfer, merge, sync_total
//! - Safety metrics: snapshot_blocked (I5), buffer_drops (I6), verification_failures (I7)
//!
//! See [`metrics`] module for trait definition and [`prometheus_metrics`] for production use.

mod blobs;
mod config;
mod delta_request;
mod hash_comparison;
pub mod hash_comparison_protocol;
mod helpers;
mod key;
pub mod level_sync;
mod manager;
pub mod metrics;
pub mod prometheus_metrics;
mod snapshot;
pub(crate) mod stream;
mod tracking;

pub use config::SyncConfig;
pub use hash_comparison_protocol::{
    HashComparisonConfig, HashComparisonFirstRequest, HashComparisonProtocol, HashComparisonStats,
};
pub use level_sync::{LevelWiseConfig, LevelWiseFirstRequest, LevelWiseProtocol, LevelWiseStats};
pub use manager::SyncManager;
pub use metrics::{no_op_metrics, NoOpMetrics, PhaseTimer, SharedMetrics, SyncMetricsCollector};
pub use prometheus_metrics::PrometheusSyncMetrics;

pub use key::CHALLENGE_DOMAIN;
