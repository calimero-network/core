use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// Private entry points the fuzz targets drive, compiled only under `cargo fuzz`.
#[cfg(fuzzing)]
#[doc(hidden)]
pub mod fuzz_api {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;

    pub fn extract_bundle_files<'a>(
        bundle_data: &[u8],
        wanted: &BTreeSet<&'a str>,
    ) -> eyre::Result<HashMap<&'a str, Arc<[u8]>>> {
        crate::client::application::bundle::extract_bundle_files(bundle_data, wanted)
    }
}

pub use calimero_bundle as bundle;
pub mod client;
pub use client::{BlobManager, SyncClient};
pub mod dag_compaction;
pub use dag_compaction::DagCompactionConfig;
pub mod tombstone_gc;
pub use tombstone_gc::GcConfig;
pub mod delta_buffer;
pub mod join_bundle;
pub use join_bundle::JoinBundle;
pub mod messages;
pub mod presence;
pub mod sync;
pub mod sync_status;
#[cfg(any(test, feature = "testing"))]
pub mod test_fixtures;
pub use sync_status::{SyncState, SyncStatusSnapshot};
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
