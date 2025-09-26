#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]

use calimero_context_config::client::config::ClientConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ContextConfig {
    #[serde(rename = "config")]
    pub client: ClientConfig,
}
