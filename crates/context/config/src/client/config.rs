#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]
use std::collections::BTreeMap;

use near_primitives::types::AccountId;
use serde::{Deserialize, Serialize};
use url::Url;

use super::{near, starknet};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigClientConfig {
    pub new: ContextConfigClientNew,
    pub signer: ContextConfigClientSigner,
}

#[non_exhaustive]
#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Near,
    Starknet,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigClientNew {
    pub protocol: Protocol,
    pub network: String,
    pub contract_id: AccountId,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigClientSigner {
    #[serde(rename = "use")]
    pub selected: ContextConfigClientSelectedSigner,
    pub relayer: ContextConfigClientRelayerSigner,
    #[serde(rename = "self")]
    pub local: BTreeMap<String, ContextConfigClientLocalSigner>,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ContextConfigClientSelectedSigner {
    Relayer,
    #[serde(rename = "self")]
    Local,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigClientRelayerSigner {
    pub url: Url,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigClientLocalSigner {
    pub rpc_url: Url,
    #[serde(flatten)]
    pub credentials: Credentials,
}

#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Credentials {
    Near(near::Credentials),
    Starknet(starknet::Credentials),
}
