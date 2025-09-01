#![cfg(feature = "client")]
#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use url::Url;

#[cfg(feature = "ethereum_client")]
use crate::client::protocol::ethereum::Credentials as EthereumCredentials;
#[cfg(feature = "icp_client")]
use crate::client::protocol::icp::Credentials as IcpCredentials;
#[cfg(feature = "near_client")]
use crate::client::protocol::near::Credentials as NearCredentials;
#[cfg(feature = "starknet_client")]
use crate::client::protocol::starknet::Credentials as StarknetCredentials;

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
    pub relayer: ClientRelayerSigner,
    #[serde(rename = "self")]
    pub local: LocalConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalConfig {
    #[serde(flatten)]
    pub protocols: BTreeMap<String, ClientLocalConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientLocalConfig {
    #[serde(flatten)]
    pub signers: BTreeMap<String, ClientLocalSigner>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientRelayerSigner {
    pub url: Url,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientLocalSigner {
    pub rpc_url: Url,
    #[serde(flatten)]
    pub credentials: Credentials,
}

#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Credentials {
    #[cfg(feature = "near_client")]
    Near(NearCredentials),
    #[cfg(feature = "starknet_client")]
    Starknet(StarknetCredentials),
    #[cfg(feature = "icp_client")]
    Icp(IcpCredentials),
    #[cfg(feature = "ethereum_client")]
    Ethereum(EthereumCredentials),
    Raw(RawCredentials),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RawCredentials {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub public_key: String,
    pub secret_key: String,
}
