#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]
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
    pub credentials: CryptoCredentials,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "serde_creds::Credentials")]
pub struct Credentials {
    pub account_id: AccountId,
    pub public_key: PublicKey,
    pub secret_key: SecretKey,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "sn_serde_creds::Credentials")]
pub struct SnCredentials {
    pub account_id: String,
    pub public_key: String,
    pub secret_key: String,
}

#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CryptoCredentials {
    Near(Credentials),
    Starknet(SnCredentials),
}

mod serde_creds {
    use near_crypto::{PublicKey, SecretKey};
    use near_primitives::types::AccountId;
    use serde::{Deserialize, Serialize};

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
                let Ok(public_key) = PublicKey::from_near_implicit_account(&creds.account_id)
                else {
                    return Err("fatal: failed to derive public key from implicit account ID");
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

mod sn_serde_creds {
    use std::str::FromStr;

    use serde::{Deserialize, Serialize};
    use starknet_crypto::Felt;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Credentials {
        secret_key: String,
        public_key: String,
        account_id: String,
    }

    impl TryFrom<Credentials> for super::SnCredentials {
        type Error = &'static str;

        fn try_from(creds: Credentials) -> Result<Self, Self::Error> {
            'pass: {
                let public_key_felt = Felt::from_str(&creds.public_key)
                    .map_err(|_| "Failed to convert public_key to Felt")?;
                let secret_key_felt = Felt::from_str(&creds.secret_key)
                    .map_err(|_| "Failed to convert secret_key to Felt")?;
                let extracted_public_key = starknet_crypto::get_public_key(&secret_key_felt);

                if public_key_felt != extracted_public_key {
                    return Err(
                        "public key extracted from private key does not match provided public key"
                            .into(),
                    );
                }

                break 'pass;
            };

            Ok(Self {
                account_id: creds.account_id,
                public_key: creds.public_key,
                secret_key: creds.secret_key,
            })
        }
    }
}
