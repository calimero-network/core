#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]
use std::collections::BTreeMap;
use std::str::FromStr;

use clap::ValueEnum;
use near_primitives::types::AccountId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use super::{near, starknet};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigClientConfig {
    pub new: ContextConfigClientNew,
    pub signer: ContextConfigClientSigner,
}

#[non_exhaustive]
#[derive(Copy, Clone, Debug, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Near,
    Starknet,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Near => "near",
            Protocol::Starknet => "starknet",
        }
    }
}

#[derive(Debug, Error, Copy, Clone)]
#[error("Failed to parse protocol")]
pub struct ProtocolParseError {
    _priv: (),
}

impl FromStr for Protocol {
    type Err = ProtocolParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.to_lowercase().as_str() {
            "near" => Ok(Protocol::Near),
            "starknet" => Ok(Protocol::Starknet),
            _ => Err(ProtocolParseError { _priv: () }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigClientNew {
    pub protocol: Protocol,
    pub network: String,
    pub contract_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalConfig {
    pub near: BTreeMap<String, ContextConfigClientLocalSigner>,
    pub starknet: BTreeMap<String, ContextConfigClientLocalSigner>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigClientSigner {
    #[serde(rename = "use")]
    pub selected: ContextConfigClientSelectedSigner,
    pub relayer: ContextConfigClientRelayerSigner,
    #[serde(rename = "self")]
    pub local: LocalConfig,
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
