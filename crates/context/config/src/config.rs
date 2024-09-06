use std::collections::BTreeMap;

use near_crypto::SecretKey;
use near_primitives::types::AccountId;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigConfig {
    pub signer: ContextConfigSigner,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigSigner {
    #[serde(rename = "use")]
    pub selected: ContextConfigSelectedSigner,
    pub relayer: ContextConfigRelayerSigner,
    #[serde(rename = "self")]
    pub local: BTreeMap<String, ContextConfigLocalSigner>,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextConfigSelectedSigner {
    Relayer,
    #[serde(rename = "self")]
    Local,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigRelayerSigner {
    pub url: Url,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigLocalSigner {
    pub rpc_url: Url,
    pub account_id: AccountId,
    pub secret_key: SecretKey,
}
