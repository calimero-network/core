use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub mod bundle;
pub mod client;
pub mod delta_buffer;
pub mod messages;
pub mod sync;

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

/// Ed25519 keypair used as the node's group identity for signing group
/// contract operations, decoupled from the NEAR signer key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupIdentityConfig {
    pub public_key: String,
    pub secret_key: String,
}
