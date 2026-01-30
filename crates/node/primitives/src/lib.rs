use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub mod bundle;
pub mod client;
pub mod messages;
pub mod sync;
pub mod sync_protocol;

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
