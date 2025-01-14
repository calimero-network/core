#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::client::protocol::icp::Credentials as IcpCredentials;
use crate::client::protocol::near::Credentials as NearCredentials;
use crate::client::protocol::starknet::Credentials as StarknetCredentials;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientConfig {
    pub near: ClientNew,
    pub starknet: ClientNew,
    pub icp: ClientNew,
    pub signer: ClientSigner,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientNew {
    pub protocol: String,
    pub network: String,
    pub contract_id: String,
    pub signer: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalConfig {
    pub near: BTreeMap<String, ClientLocalSigner>,
    pub starknet: BTreeMap<String, ClientLocalSigner>,
    pub icp: BTreeMap<String, ClientLocalSigner>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientSigner {
    pub relayer: ClientRelayerSigner,
    #[serde(rename = "self")]
    pub local: LocalConfig,
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
    Near(NearCredentials),
    Starknet(StarknetCredentials),
    Icp(IcpCredentials),
}
