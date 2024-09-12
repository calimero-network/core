use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

#[cfg(feature = "rand")]
use rand::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::context::ContextId;
use crate::hash::{Hash, HashError};

#[derive(Eq, Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrivateKey(Hash);

impl From<[u8; 32]> for PrivateKey {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
    }
}

impl Deref for PrivateKey {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PrivateKey {
    pub fn public_key(&self) -> PublicKey {
        ed25519_dalek::SigningKey::from_bytes(&self)
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

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for PrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl From<PrivateKey> for String {
    fn from(id: PrivateKey) -> Self {
        id.as_str().to_owned()
    }
}

impl From<&PrivateKey> for String {
    fn from(id: &PrivateKey) -> Self {
        id.as_str().to_owned()
    }
}

#[derive(Clone, Copy, Debug, Error)]
#[error(transparent)]
pub struct InvalidPrivateKey(HashError);

impl FromStr for PrivateKey {
    type Err = InvalidPrivateKey;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidPrivateKey)?))
    }
}

#[derive(Eq, Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PublicKey(Hash);

impl From<[u8; 32]> for PublicKey {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
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

pub mod serde_identity {
    use core::fmt::{self, Formatter};

    use libp2p_identity::Keypair;
    use serde::de::{self, MapAccess};
    use serde::ser::{self, SerializeMap};
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(key: &Keypair, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut keypair = serializer.serialize_map(Some(2))?;
        keypair.serialize_entry("peer_id", &key.public().to_peer_id().to_base58())?;
        keypair.serialize_entry(
            "keypair",
            &bs58::encode(&key.to_protobuf_encoding().map_err(ser::Error::custom)?).into_string(),
        )?;
        keypair.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Keypair, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IdentityVisitor;

        impl<'de> de::Visitor<'de> for IdentityVisitor {
            type Value = Keypair;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                formatter.write_str("an identity")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut peer_id = None::<String>;
                let mut priv_key = None::<String>;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "peer_id" => peer_id = Some(map.next_value()?),
                        "keypair" => priv_key = Some(map.next_value()?),
                        _ => {
                            drop(map.next_value::<de::IgnoredAny>());
                        }
                    }
                }

                let peer_id = peer_id.ok_or_else(|| de::Error::missing_field("peer_id"))?;
                let priv_key = priv_key.ok_or_else(|| de::Error::missing_field("keypair"))?;

                let priv_key = bs58::decode(priv_key)
                    .into_vec()
                    .map_err(|_| de::Error::custom("invalid base58"))?;

                let keypair = Keypair::from_protobuf_encoding(&priv_key)
                    .map_err(|_| de::Error::custom("invalid protobuf"))?;

                if peer_id != keypair.public().to_peer_id().to_base58() {
                    return Err(de::Error::custom("Peer id does not match public key"));
                }

                Ok(keypair)
            }
        }

        deserializer.deserialize_struct("Keypair", &["peer_id", "keypair"], IdentityVisitor)
    }
}
