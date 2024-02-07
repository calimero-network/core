use std::fmt;
use std::fs;

use color_eyre::eyre::{self, Context};
use libp2p::identity;
use libp2p::multiaddr::{self, Multiaddr};
use serde::{Deserialize, Serialize};

const CONFIG_FILE: &str = "config.toml";
pub const DEFAULT_PORT: u16 = 2428;
pub const DEFAULT_RPC_HOST: &str = "127.0.0.1";
pub const DEFAULT_RPC_PORT: u16 = 3030;
pub const DEFAULT_CALIMERO_CHAT_HOME: &str = ".calimero/experiments/chat-p0c";

// https://github.com/ipfs/kubo/blob/efdef7fdcfeeb30e2f1ce3dbf65b6460b58afaaf/config/bootstrap_peers.go#L17-L24
pub const DEFAULT_BOOTSTRAP_NODES: &[&str] = &[
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
    "/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
    "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
];

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(
        with = "serde_identity",
        default = "identity::Keypair::generate_ed25519"
    )]
    pub identity: identity::Keypair,
    pub swarm: SwarmConfig,
    #[serde(default)]
    pub bootstrap: BootstrapConfig,
    #[serde(default)]
    pub discovery: DiscoveryConfig,
    #[serde(default)]
    pub endpoint: EndpointConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SwarmConfig {
    pub listen: Vec<Multiaddr>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BootstrapConfig {
    #[serde(default = "default_bootstrap")]
    #[serde(deserialize_with = "deserialize_bootstrap")]
    pub nodes: Vec<Multiaddr>,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            nodes: default_bootstrap(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    #[serde(default = "bool_true")]
    pub mdns: bool,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self { mdns: bool_true() }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EndpointConfig {
    pub host: String,
    pub port: u16,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_RPC_HOST.to_string(),
            port: DEFAULT_RPC_PORT,
        }
    }
}

impl Config {
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

pub fn default_bootstrap() -> Vec<Multiaddr> {
    DEFAULT_BOOTSTRAP_NODES
        .iter()
        .map(|s| s.parse().expect("invalid multiaddr"))
        .collect()
}

const fn bool_true() -> bool {
    true
}

fn deserialize_bootstrap<'de, D>(deserializer: D) -> Result<Vec<Multiaddr>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct BootstrapVisitor;

    impl<'de> de::Visitor<'de> for BootstrapVisitor {
        type Value = Vec<Multiaddr>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a list of multiaddresses")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut addrs = Vec::new();

            while let Some(addr) = seq.next_element::<Multiaddr>()? {
                let Some(multiaddr::Protocol::P2p(_)) = addr.iter().last() else {
                    return Err(serde::de::Error::custom("peer ID not allowed"));
                };

                addrs.push(addr);
            }

            Ok(addrs)
        }
    }

    deserializer.deserialize_seq(BootstrapVisitor)
}

mod serde_identity {
    use std::fmt;

    use libp2p::identity::Keypair;
    use serde::{
        de::{self, MapAccess},
        ser::{self, SerializeMap},
        Deserializer, Serializer,
    };

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
