use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub mod bundle;
pub mod client;
pub use client::{BlobManager, SyncClient};
pub mod dag_compaction;
pub use dag_compaction::DagCompactionConfig;
pub mod delta_buffer;
pub mod join_bundle;
pub use join_bundle::JoinBundle;
pub mod messages;
pub mod sync;
pub mod sync_status;
pub use sync_status::{SyncPhase, SyncStatusSnapshot};
pub mod topic_manager;
pub use topic_manager::TopicManager;

/// Node operation mode
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum NodeMode {
    /// Standard mode - full node functionality with JSON-RPC execution
    #[default]
    Standard,
    /// Read-only mode - disables JSON-RPC execution, used for TEE observer nodes
    ReadOnly,
}
