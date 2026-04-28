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
pub(crate) mod helpers;
pub mod level_sync;
mod manager;
pub mod metrics;
pub(crate) mod parent_pull;
pub mod prometheus_metrics;
pub(crate) mod rotation_log_reader;
mod snapshot;
pub(crate) mod stream;
mod tracking;

// Cross-node integration tests for the four motivating partition scenarios
// of #2197 / ADR 0001. Migrated from `calimero_storage::tests` per #2266
// step 5 — they exercise the production sync-layer flow: load rotation log,
// resolve `effective_writers` via `rotation_log_reader::writers_at`, apply.
#[cfg(test)]
mod p3_dag_causal_tests;
#[cfg(test)]
mod p5_partition_scenarios_tests;

pub use config::SyncConfig;
pub use hash_comparison_protocol::{
    HashComparisonConfig, HashComparisonFirstRequest, HashComparisonProtocol, HashComparisonStats,
};
pub use level_sync::{LevelWiseConfig, LevelWiseFirstRequest, LevelWiseProtocol, LevelWiseStats};
pub use manager::SyncManager;
pub use metrics::{no_op_metrics, NoOpMetrics, PhaseTimer, SharedMetrics, SyncMetricsCollector};
pub use prometheus_metrics::PrometheusSyncMetrics;
