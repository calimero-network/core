#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use url::Url;

#[cfg(feature = "near_client")]
use crate::client::protocol::near::Credentials as NearCredentials;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientConfig {
    #[serde(flatten)]
    pub params: BTreeMap<String, ClientConfigParams>,
    pub signer: ClientSigner,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientConfigParams {
    pub signer: ClientSelectedSigner,
    pub network: String,
    pub contract_id: String,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ClientSelectedSigner {
    Relayer,
    #[serde(rename = "self")]
    Local,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientSigner {
    /// Relayer HTTP endpoint for chain-backed context operations. Omitted for pure **local**
    /// group governance (`merod init --group-governance local`); deserialization defaults to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relayer: Option<ClientRelayerSigner>,
    #[serde(rename = "self")]
    pub local: LocalConfig,
}

#[cfg(feature = "near_client")]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalConfig {
    #[serde(flatten)]
    pub protocols: BTreeMap<String, ClientLocalConfig>,
}

/// When `near_client` is disabled, only empty `protocols` are accepted (relayer-only stack).
#[cfg(not(feature = "near_client"))]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalConfig {
    #[serde(default)]
    pub protocols: BTreeMap<String, serde_json::Value>,
}

#[cfg(feature = "near_client")]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientLocalConfig {
    #[serde(flatten)]
    pub signers: BTreeMap<String, ClientLocalSigner>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientRelayerSigner {
    pub url: Url,
}

#[cfg(feature = "near_client")]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientLocalSigner {
    pub rpc_url: Url,
    #[serde(flatten)]
    pub credentials: Credentials,
}

#[cfg(feature = "near_client")]
#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Credentials {
    Near(NearCredentials),
}
