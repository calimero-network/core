use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::context::ContextId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::exhaustive_structs)]
pub struct KeyPair {
    pub public_key: PublicKey,
    pub private_key: Option<[u8; 32]>,
}

// This could use a Hash, but we need to be able to serialize the PublicKey and
// create::hash::Hash does not currently implement Borsh.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[allow(clippy::exhaustive_structs)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    #[must_use]
    pub fn derive_from_private_key(private_key: &[u8; 32]) -> Self {
        let secret_key = SigningKey::from_bytes(private_key);
        let public_key: VerifyingKey = (&secret_key).into();
        public_key.into()
    }
}

impl From<[u8; 32]> for PublicKey {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<VerifyingKey> for PublicKey {
    fn from(public_key: VerifyingKey) -> Self {
        Self(public_key.to_bytes())
    }
}
impl From<KeyPair> for PublicKey {
    fn from(key_pair: KeyPair) -> Self {
        key_pair.public_key
    }
}

impl From<&KeyPair> for PublicKey {
    fn from(key_pair: &KeyPair) -> Self {
        key_pair.public_key
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
    pub created_at: u64,
}

impl RootKey {
    #[must_use]
    pub const fn new(signing_key: String, wallet_type: WalletType, created_at: u64) -> Self {
        Self {
            signing_key,
            wallet_type,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum WalletType {
    NEAR,
    ETH {
        #[serde(rename = "chainId")]
        chain_id: u64,
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
