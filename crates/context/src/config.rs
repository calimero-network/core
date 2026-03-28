#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]

pub use calimero_context_config::client_config::ClientConfig;
use serde::{Deserialize, Serialize};

/// Node context section: client config only (local group governance; no chain).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfig {
    #[serde(rename = "config")]
    pub client: ClientConfig,
}
