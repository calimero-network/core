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
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClientKey {
    pub wallet_type: WalletType,
    pub signing_key: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub id: String,
    #[serde(
        with = "serde_identity",
        default = "libp2p::identity::Keypair::generate_ed25519"
    )]
    pub identity: libp2p::identity::Keypair,
    pub application_id: String,
}

#[derive(Debug, Deserialize, PartialEq, Serialize, Clone, Copy)]
pub enum WalletType {
    NEAR,
    ETH,
}

impl WalletType {
    pub fn from_str(input: &str) -> eyre::Result<Self> {
        match input {
            "ETH" => Ok(WalletType::ETH),
            "NEAR" => Ok(WalletType::NEAR),
            _ => eyre::bail!("Invalid wallet_type value"),
        }
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
        keypair.serialize_entry("peerId", &key.public().to_peer_id().to_base58())?;
        keypair.serialize_entry(
            "privateKey",
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
                        "peerId" => peer_id = Some(map.next_value()?),
                        "privateKey" => priv_key = Some(map.next_value()?),
                        _ => {
                            let _ = map.next_value::<de::IgnoredAny>();
                        }
                    }
                }

                let peer_id = peer_id.ok_or_else(|| de::Error::missing_field("peerId"))?;
                let priv_key = priv_key.ok_or_else(|| de::Error::missing_field("privateKey"))?;

                let priv_key = bs58::decode(priv_key)
                    .into_vec()
                    .map_err(|_| de::Error::custom("invalid base58"))?;

                let keypair = Keypair::from_protobuf_encoding(&priv_key)
                    .map_err(|_| de::Error::custom("invalid protobuf"))?;

                if peer_id != keypair.public().to_peer_id().to_base58() {
                    return Err(de::Error::custom("PeerID does not match public key"));
                }

                Ok(keypair)
            }
        }

        deserializer.deserialize_struct("Keypair", &["peerId", "privateKey"], IdentityVisitor)
    }
}
