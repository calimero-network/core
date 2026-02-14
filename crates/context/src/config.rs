#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]

use calimero_context_config::client::config::ClientConfig;
use serde::{Deserialize, Serialize};

/// Default maximum number of concurrent WASM executions.
/// This provides a reasonable default that prevents resource exhaustion
/// while allowing sufficient parallelism for typical workloads.
pub const DEFAULT_MAX_CONCURRENT_EXECUTIONS: usize = 16;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfig {
    #[serde(rename = "config")]
    pub client: ClientConfig,

    /// Maximum number of concurrent WASM executions allowed.
    /// When this limit is reached, new execution requests will be queued
    /// until a slot becomes available. This prevents resource exhaustion
    /// under high load.
    ///
    /// Default: 16
    #[serde(default = "default_max_concurrent_executions")]
    pub max_concurrent_executions: usize,
}

fn default_max_concurrent_executions() -> usize {
    DEFAULT_MAX_CONCURRENT_EXECUTIONS
}
