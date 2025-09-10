use core::time::Duration;

use calimero_context::config::ContextConfig;
use calimero_network_primitives::config::{BootstrapConfig, DiscoveryConfig, SwarmConfig};
use calimero_server::admin::service::AdminConfig;
use calimero_server::jsonrpc::JsonRpcConfig;
use calimero_server::ws::WsConfig;
use camino::{Utf8Path, Utf8PathBuf};
use eyre::{Result as EyreResult, WrapErr};
use multiaddr::Multiaddr;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs::{read_to_string, write};

use crate::serde_duration::DurationSchema;
use crate::serde_identity::KeypairSchema;

pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct ConfigFile {
    #[serde(
        with = "serde_identity",
        default = "libp2p_identity::Keypair::generate_ed25519"
    )]
    #[schemars(with = "KeypairSchema")]
    pub identity: libp2p_identity::Keypair,

    #[serde(flatten)]
    pub network: NetworkConfig,

    pub sync: SyncConfig,

    pub datastore: DataStoreConfig,

    pub blobstore: BlobStoreConfig,

    pub context: ContextConfig,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocols: Option<ProtocolsConfig>,

    // Support both old and new format during migration
    #[serde(rename = "context", default, skip_serializing_if = "Option::is_none")]
    pub old_context: Option<OldContextConfig>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct OldContextConfig {
    #[serde(rename = "config", default, skip_serializing_if = "Option::is_none")]
    pub old_protocols: Option<OldProtocolsConfig>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct OldProtocolsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ethereum: Option<EthereumProtocolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub near: Option<NearProtocolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icp: Option<IcpProtocolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stellar: Option<StellarProtocolConfig>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct ProtocolsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ethereum: Option<EthereumProtocolConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub icp: Option<IcpProtocolConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub near: Option<NearProtocolConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub stellar: Option<StellarProtocolConfig>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct EthereumProtocolConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
}

// Apply the same pattern to other protocol config structs:
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct IcpProtocolConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct NearProtocolConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct StellarProtocolConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SyncConfig {
    #[serde(rename = "timeout_ms", with = "serde_duration")]
    #[schemars(with = "DurationSchema")]
    pub timeout: Duration,
    #[serde(rename = "interval_ms", with = "serde_duration")]
    #[schemars(with = "DurationSchema")]
    pub interval: Duration,
    #[serde(rename = "frequency_ms", with = "serde_duration")]
    #[schemars(with = "DurationSchema")]
    pub frequency: Duration,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct NetworkConfig {
    pub swarm: SwarmConfig,

    pub server: ServerConfig,

    #[serde(default)]
    pub bootstrap: BootstrapConfig,

    #[serde(default)]
    pub discovery: DiscoveryConfig,
}

impl NetworkConfig {
    #[must_use]
    pub const fn new(
        swarm: SwarmConfig,
        bootstrap: BootstrapConfig,
        discovery: DiscoveryConfig,
        server: ServerConfig,
    ) -> Self {
        Self {
            swarm,
            server,
            bootstrap,
            discovery,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct ServerConfig {
    #[schemars(with = "MultiaddrWrapper")]
    pub listen: Vec<Multiaddr>,

    #[serde(default)]
    pub admin: Option<AdminConfig>,

    #[serde(default)]
    pub jsonrpc: Option<JsonRpcConfig>,

    #[serde(default)]
    pub websocket: Option<WsConfig>,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug)]
#[serde(transparent)]
#[schemars(description = "A libp2p multiaddress string")]
pub struct MultiaddrWrapper(#[schemars(with = "String")] pub Multiaddr);

impl ServerConfig {
    #[must_use]
    pub const fn new(
        listen: Vec<Multiaddr>,
        admin: Option<AdminConfig>,
        jsonrpc: Option<JsonRpcConfig>,
        websocket: Option<WsConfig>,
    ) -> Self {
        Self {
            listen,
            admin,
            jsonrpc,
            websocket,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct DataStoreConfig {
    #[schemars(with = "Utf8PathBufWrapper")]
    pub path: Utf8PathBuf,
}

impl DataStoreConfig {
    #[must_use]
    pub const fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct BlobStoreConfig {
    #[schemars(with = "Utf8PathBufWrapper")]
    pub path: Utf8PathBuf,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug)]
#[serde(transparent)]
#[schemars(description = "A libp2p multiaddress string")]
pub struct Utf8PathBufWrapper(#[schemars(with = "String")] pub Utf8PathBuf);

impl BlobStoreConfig {
    #[must_use]
    pub const fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}

impl ConfigFile {
    #[must_use]
    pub const fn new(
        identity: libp2p_identity::Keypair,
        network: NetworkConfig,
        sync: SyncConfig,
        datastore: DataStoreConfig,
        blobstore: BlobStoreConfig,
        context: ContextConfig,
    ) -> Self {
        Self {
            identity,
            network,
            sync,
            datastore,
            blobstore,
            context,
            protocols: None,
            old_context: None,
        }
    }

    #[must_use]
    pub fn exists(dir: &Utf8Path) -> bool {
        dir.join(CONFIG_FILE).is_file()
    }

    pub async fn load(dir: &Utf8Path) -> EyreResult<Self> {
        let path = dir.join(CONFIG_FILE);
        let content = read_to_string(&path).await.wrap_err_with(|| {
            format!(
                "failed to read configuration from {:?}",
                dir.join(CONFIG_FILE)
            )
        })?;

        let mut config: Self = toml::from_str(&content)?;

        // Migrate old context.config.* format to new protocols.* format
        if let Some(old_context) = config.old_context.take() {
            if let Some(old_protocols) = old_context.old_protocols {
                if config.protocols.is_none() {
                    config.protocols = Some(ProtocolsConfig {
                        ethereum: old_protocols.ethereum,
                        near: old_protocols.near,
                        icp: old_protocols.icp,
                        stellar: old_protocols.stellar,
                    });
                }
            }
        }

        Ok(config)
    }

    pub async fn save(&mut self, dir: &Utf8Path) -> EyreResult<()> {
        let path = dir.join(CONFIG_FILE);

        // Create a copy without the old context field for serialization
        let config_for_serialization = self;
        config_for_serialization.old_context = None;

        let content = toml::to_string_pretty(&config_for_serialization)?;

        write(&path, content).await.wrap_err_with(|| {
            format!(
                "failed to write configuration to {:?}",
                dir.join(CONFIG_FILE)
            )
        })?;

        Ok(())
    }
}

mod serde_duration {
    use core::time::Duration;

    use schemars::JsonSchema;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_millis() as u64)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(Duration::from_millis)
    }

    #[derive(JsonSchema)]
    #[serde(transparent)]
    pub struct DurationSchema(#[schemars(with = "u64")] pub Duration);
}

pub mod serde_identity {
    use core::fmt::{self, Formatter};

    use libp2p_identity::Keypair;
    use schemars::JsonSchema;
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
                    return Err(de::Error::custom("Peer ID does not match public key"));
                }

                Ok(keypair)
            }
        }

        deserializer.deserialize_struct("Keypair", &["peer_id", "keypair"], IdentityVisitor)
    }

    #[derive(JsonSchema, Debug)]
    #[serde(transparent)]
    pub struct KeypairSchema(#[schemars(with = "String")] pub Keypair);
}
