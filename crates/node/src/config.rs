use std::fs;

use eyre::WrapErr;
use libp2p::identity;
use serde::{Deserialize, Serialize};

const CONFIG_FILE: &str = "config.toml";

pub const DEFAULT_CALIMERO_CHAT_HOME: &str = ".calimero/experiments/chat-p0c";

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(
        with = "serde_identity",
        default = "identity::Keypair::generate_ed25519"
    )]
    pub identity: identity::Keypair,

    #[serde(flatten)]
    pub network: NetworkConfig,

    pub store: StoreConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub swarm: calimero_network::config::SwarmConfig,

    pub server: calimero_server::config::ServerConfig,

    #[serde(default)]
    pub bootstrap: calimero_network::config::BootstrapConfig,

    #[serde(default)]
    pub discovery: calimero_network::config::DiscoveryConfig,

    pub app: AppConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StoreConfig {
    pub path: camino::Utf8PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub wasm_path: String,
}

impl ConfigFile {
    pub fn exists(dir: &camino::Utf8Path) -> bool {
        dir.join(CONFIG_FILE).is_file()
    }

    pub fn load(dir: &camino::Utf8Path) -> eyre::Result<Self> {
        let path = dir.join(CONFIG_FILE);
        let content = fs::read_to_string(&path).wrap_err_with(|| {
            format!(
                "failed to read configuration from {:?}",
                dir.join(CONFIG_FILE)
            )
        })?;

        toml::from_str(&content).map_err(Into::into)
    }

    pub fn save(&self, dir: &camino::Utf8Path) -> eyre::Result<()> {
        let path = dir.join(CONFIG_FILE);
        let content = toml::to_string_pretty(self)?;

        fs::write(&path, content).wrap_err_with(|| {
            format!(
                "failed to write configuration to {:?}",
                dir.join(CONFIG_FILE)
            )
        })?;

        Ok(())
    }
}

pub fn default_chat_dir() -> camino::Utf8PathBuf {
    if let Some(home) = dirs::home_dir() {
        let home = camino::Utf8Path::from_path(&home).expect("invalid home directory");
        return home.join(DEFAULT_CALIMERO_CHAT_HOME);
    }

    Default::default()
}

mod serde_identity {
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
        keypair.serialize_entry("PeerID", &key.public().to_peer_id().to_base58())?;
        keypair.serialize_entry(
            "PrivKey",
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
                        "PeerID" => peer_id = Some(map.next_value()?),
                        "PrivKey" => priv_key = Some(map.next_value()?),
                        _ => {
                            let _ = map.next_value::<de::IgnoredAny>();
                        }
                    }
                }

                let peer_id = peer_id.ok_or_else(|| de::Error::missing_field("PeerID"))?;
                let priv_key = priv_key.ok_or_else(|| de::Error::missing_field("PrivKey"))?;

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

        deserializer.deserialize_struct("Keypair", &["PeerID", "PrivKey"], IdentityVisitor)
    }
}
