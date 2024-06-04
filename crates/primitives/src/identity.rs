use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Did {
    pub id: String,
    pub root_keys: Vec<RootKey>,
    pub client_keys: Vec<ClientKey>,
    pub contexts: Vec<Context>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RootKey {
    pub signing_key: String,
    #[serde(flatten)]
    pub wallet_type: WalletType,
    pub created_at: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClientKey {
    #[serde(flatten)]
    pub wallet_type: WalletType,
    pub signing_key: String,
    pub created_at: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub id: String,
    #[serde(with = "serde_signing_key")]
    pub signing_key: ed25519_dalek::SigningKey,
    pub application_id: String,
}

#[derive(Debug, Deserialize, PartialEq, Serialize, Clone, Copy)]
#[serde(rename_all = "UPPERCASE")]
#[serde(tag = "type")]
pub enum WalletType {
    NEAR,
    ETH {
        #[serde(rename = "chainId")]
        chain_id: u64,
    },
}

pub mod serde_signing_key {
    use std::fmt;

    use ed25519_dalek::SigningKey;
    use serde::de::{self, MapAccess, Visitor};
    use serde::ser::{SerializeMap, Serializer};
    use serde::Deserializer;

    pub fn serialize<S>(key: &SigningKey, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(1))?;
        let key_bytes = key.to_bytes();
        let encoded_key = bs58::encode(key_bytes).into_string();
        map.serialize_entry("signingKey", &encoded_key)?;
        map.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<SigningKey, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SigningKeyVisitor;

        impl<'de> Visitor<'de> for SigningKeyVisitor {
            type Value = SigningKey;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a signing key")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut signing_key = None::<String>;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "signingKey" => signing_key = Some(map.next_value()?),
                        _ => {
                            let _ = map.next_value::<de::IgnoredAny>();
                        }
                    }
                }

                let signing_key =
                    signing_key.ok_or_else(|| de::Error::missing_field("signingKey"))?;
                let decoded_key = bs58::decode(signing_key)
                    .into_vec()
                    .map_err(|_| de::Error::custom("invalid base58"))?;

                let array: [u8; 32] = decoded_key
                    .as_slice()
                    .try_into()
                    .map_err(|_| de::Error::custom("invalid signing key"))?;

                let signing_key = SigningKey::from_bytes(&array);

                Ok(signing_key)
            }
        }

        deserializer.deserialize_struct("SigningKey", &["signingKey"], SigningKeyVisitor)
    }
}

pub mod serde_identity {
    use std::fmt;

    use libp2p::identity::Keypair;
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

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
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
                            let _ = map.next_value::<de::IgnoredAny>();
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
