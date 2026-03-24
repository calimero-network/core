#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]

use calimero_context_config::client::config::ClientConfig;
use serde::{Deserialize, Serialize};

/// Where context **group** policy is sourced: NEAR / relayer (`External`) or signed P2P ops (`Local`).
#[derive(Copy, Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GroupGovernanceMode {
    #[default]
    External,
    Local,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfig {
    #[serde(rename = "config")]
    pub client: ClientConfig,
    #[serde(default)]
    pub group_governance: GroupGovernanceMode,
}
