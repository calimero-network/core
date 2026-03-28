//! Node `[context.config]` in `config.toml` — local-only (no chain transports).

#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Top-level context client section in node configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientConfig {
    pub signer: ClientSigner,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientSigner {
    #[serde(rename = "self")]
    pub local: LocalConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalConfig {
    #[serde(default)]
    pub protocols: BTreeMap<String, serde_json::Value>,
}
