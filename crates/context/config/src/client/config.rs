use std::collections::BTreeMap;

use near_crypto::{PublicKey, SecretKey};
use near_primitives::types::AccountId;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigClientConfig {
    pub new: ContextConfigClientNew,
    pub signer: ContextConfigClientSigner,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfigClientNew {
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "serde_creds::Credentials")]
pub struct Credentials {
    pub account_id: AccountId,
    pub public_key: PublicKey,
    pub secret_key: SecretKey,
}

mod serde_creds {

    use super::*;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Credentials {
        account_id: AccountId,
        public_key: PublicKey,
        secret_key: SecretKey,
    }

    impl TryFrom<Credentials> for super::Credentials {
        type Error = &'static str;

        fn try_from(creds: Credentials) -> Result<Self, Self::Error> {
            'pass: {
                if let SecretKey::ED25519(key) = &creds.secret_key {
                    let mut buf = [0; 32];

                    buf.copy_from_slice(&key.0[..32]);

                    if ed25519_dalek::SigningKey::from_bytes(&buf)
                        .verifying_key()
                        .as_bytes()
                        == &key.0[32..]
                    {
                        break 'pass;
                    }
                } else if creds.public_key == creds.secret_key.public_key() {
                    break 'pass;
                }

                return Err("public key and secret key do not match");
            };

            if creds.account_id.get_account_type().is_implicit() {
                let public_key = match PublicKey::from_near_implicit_account(&creds.account_id) {
                    Ok(key) => key,
                    Err(_) => {
                        return Err("fatal: failed to derive public key from implicit account ID")
                    }
                };

                if creds.public_key != public_key {
                    return Err("implicit account ID and public key do not match");
                }
            }

            Ok(Self {
                account_id: creds.account_id,
                public_key: creds.public_key,
                secret_key: creds.secret_key,
            })
        }
    }
}
