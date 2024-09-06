use std::collections::BTreeMap;

use near_crypto::SecretKey;
use near_primitives::types::AccountId;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigClientConfig {
    pub signer: ContextConfigClientSigner,
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
    pub account_id: AccountId,
    pub secret_key: SecretKey,
}
