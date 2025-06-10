use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

#[cfg(feature = "rand")]
use rand::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::context::ContextId;
use crate::hash::{Hash, HashError};

#[expect(
    missing_copy_implementations,
    reason = "PrivateKey must not be copied, cloned, viewed or serialized"
)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
pub struct PrivateKey(Hash);

impl fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("PrivateKey")
    }
}

impl From<[u8; 32]> for PrivateKey {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
    }
}

impl AsRef<[u8; 32]> for PrivateKey {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Deref for PrivateKey {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PrivateKey {
    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        ed25519_dalek::SigningKey::from_bytes(self)
            .verifying_key()
            .to_bytes()
            .into()
    }

    #[cfg(feature = "rand")]
    pub fn random<R: CryptoRng + RngCore>(csprng: &mut R) -> Self {
        let mut secret = [0; 32];

        csprng.fill_bytes(&mut secret);

        Self::from(secret)
    }
}

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
pub struct PublicKey(Hash);

impl From<[u8; 32]> for PublicKey {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
    }
}

impl AsRef<[u8; 32]> for PublicKey {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Deref for PublicKey {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PublicKey {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl From<PublicKey> for String {
    fn from(id: PublicKey) -> Self {
        id.as_str().to_owned()
    }
}

impl From<&PublicKey> for String {
    fn from(id: &PublicKey) -> Self {
        id.as_str().to_owned()
    }
}

#[derive(Clone, Copy, Debug, Error)]
#[error(transparent)]
pub struct InvalidPublicKey(HashError);

impl FromStr for PublicKey {
    type Err = InvalidPublicKey;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidPublicKey)?))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Did {
    pub id: String,
    pub root_keys: Vec<RootKey>,
    pub client_keys: Vec<ClientKey>,
}

impl Did {
    #[must_use]
    pub const fn new(id: String, root_keys: Vec<RootKey>, client_keys: Vec<ClientKey>) -> Self {
        Self {
            id,
            root_keys,
            client_keys,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RootKey {
    pub signing_key: String,
    #[serde(rename = "wallet")]
    pub wallet_type: WalletType,
    pub wallet_address: String,
    pub created_at: u64,
}

impl RootKey {
    #[must_use]
    pub const fn new(
        signing_key: String,
        wallet_type: WalletType,
        wallet_address: String,
        created_at: u64,
    ) -> Self {
        Self {
            signing_key,
            wallet_type,
            wallet_address,
            created_at,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ClientKey {
    #[serde(rename = "wallet")]
    pub wallet_type: WalletType,
    pub signing_key: String,
    pub created_at: u64,
    pub context_id: Option<ContextId>,
}

impl ClientKey {
    #[must_use]
    pub const fn new(
        wallet_type: WalletType,
        signing_key: String,
        created_at: u64,
        context_id: Option<ContextId>,
    ) -> Self {
        Self {
            wallet_type,
            signing_key,
            created_at,
            context_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ContextUser {
    pub user_id: String,
    pub joined_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum WalletType {
    NEAR {
        #[serde(rename = "networkId")]
        network_id: NearNetworkId,
    },
    ETH {
        #[serde(rename = "chainId")]
        chain_id: u64,
    },
    STARKNET {
        #[serde(rename = "walletName")]
        wallet_name: String,
    },
    ICP {
        #[serde(rename = "canisterId")]
        canister_id: String,
        #[serde(rename = "walletName")]
        wallet_name: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum NearNetworkId {
    Mainnet,
    Testnet,
    #[serde(untagged)]
    Custom(String),
}
