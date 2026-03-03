use core::time::Duration;

use calimero_context::config::ContextConfig;
use calimero_network_primitives::config::{BootstrapConfig, DiscoveryConfig, SwarmConfig};
use calimero_server::admin::service::AdminConfig;
use calimero_server::config::AuthMode;
use calimero_server::jsonrpc::JsonRpcConfig;
use calimero_server::sse::SseConfig;
use calimero_server::ws::WsConfig;
use camino::{Utf8Path, Utf8PathBuf};
use eyre::{Result as EyreResult, WrapErr};
use multiaddr::Multiaddr;
use serde::{Deserialize, Serialize};
use tokio::fs::{read_to_string, write};
use url::Url;

use mero_auth::config::AuthConfig;

pub use calimero_node_primitives::NodeMode;

pub const CONFIG_FILE: &str = "config.toml";

/// Node identity configuration.
///
/// The keypair is stored in the datastore (encrypted when TEE is configured).
/// Config only holds `peer_id` for KMS attestation. `keypair` is optional for
/// migration from nodes that had it in config.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct IdentityConfig {
    /// Peer ID (base58) - used for KMS attestation.
    pub peer_id: String,

    /// Plaintext keypair (base58). Only present during migration from old config.
    #[serde(default)]
    pub keypair: Option<String>,
}

impl IdentityConfig {
    /// Create with peer_id only (keypair in datastore).
    #[must_use]
    pub fn peer_id_only(peer_id: String) -> Self {
        Self {
            peer_id,
            keypair: None,
        }
    }

    /// Resolve to Keypair. From config (migration) or fails if not present.
    pub fn to_keypair(&self) -> Result<libp2p_identity::Keypair, String> {
        let Some(ref kp) = self.keypair else {
            return Err("keypair not in config (expected in datastore)".to_string());
        };
        let bytes = bs58::decode(kp)
            .into_vec()
            .map_err(|_| "invalid base58 keypair")?;
        let keypair = libp2p_identity::Keypair::from_protobuf_encoding(&bytes)
            .map_err(|_| "invalid keypair encoding")?;
        if self.peer_id != keypair.public().to_peer_id().to_base58() {
            return Err("peer_id does not match keypair".to_string());
        }
        Ok(keypair)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ConfigFile {
    #[serde(with = "serde_identity")]
    pub identity: IdentityConfig,

    #[serde(default)]
    pub mode: NodeMode,

    #[serde(flatten)]
    pub network: NetworkConfig,

    pub sync: SyncConfig,

    pub datastore: DataStoreConfig,

    pub blobstore: BlobStoreConfig,

    pub context: ContextConfig,

    /// TEE-related configuration (KMS, attestation, etc.).
    #[serde(default)]
    pub tee: Option<TeeConfig>,
}

/// Configuration for TEE (Trusted Execution Environment) features.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct TeeConfig {
    /// KMS configuration for fetching storage encryption keys.
    pub kms: KmsConfig,
}

/// Configuration for the Key Management Service.
///
/// Supports multiple KMS implementations. Currently only Phala is supported.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct KmsConfig {
    /// Phala Cloud KMS configuration (mero-kms-phala).
    pub phala: Option<PhalaKmsConfig>,
}

/// Configuration for Phala Cloud KMS (mero-kms-phala).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct PhalaKmsConfig {
    /// URL of the mero-kms-phala service.
    pub url: Url,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct SyncConfig {
    #[serde(rename = "timeout_ms", with = "serde_duration")]
    pub timeout: Duration,
    #[serde(rename = "interval_ms", with = "serde_duration")]
    pub interval: Duration,
    #[serde(rename = "frequency_ms", with = "serde_duration")]
    pub frequency: Duration,
}

/// Configuration for specialized node functionality (e.g., read-only nodes).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct SpecializedNodeConfig {
    /// Topic name for specialized node invite discovery messages.
    #[serde(default = "default_specialized_node_invite_topic")]
    pub invite_topic: String,

    /// Whether to accept mock TEE attestation.
    /// WARNING: Should only be true for testing. Never enable in production!
    #[serde(default)]
    pub accept_mock_tee: bool,
}

fn default_specialized_node_invite_topic() -> String {
    "mero_specialized_node_invites".to_owned()
}

impl Default for SpecializedNodeConfig {
    fn default() -> Self {
        Self {
            invite_topic: default_specialized_node_invite_topic(),
            accept_mock_tee: false,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct NetworkConfig {
    pub swarm: SwarmConfig,

    pub server: ServerConfig,

    #[serde(default)]
    pub bootstrap: BootstrapConfig,

    #[serde(default)]
    pub discovery: DiscoveryConfig,

    /// Configuration for specialized nodes (read-only, etc.).
    #[serde(default)]
    pub specialized_node: SpecializedNodeConfig,
}

impl NetworkConfig {
    #[must_use]
    pub fn new(
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
            specialized_node: SpecializedNodeConfig::default(),
        }
    }

    /// Create a new `NetworkConfig` with custom specialized node settings.
    #[must_use]
    pub fn with_specialized_node(
        swarm: SwarmConfig,
        bootstrap: BootstrapConfig,
        discovery: DiscoveryConfig,
        server: ServerConfig,
        specialized_node: SpecializedNodeConfig,
    ) -> Self {
        Self {
            swarm,
            server,
            bootstrap,
            discovery,
            specialized_node,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ServerConfig {
    pub listen: Vec<Multiaddr>,

    #[serde(default)]
    pub admin: Option<AdminConfig>,

    #[serde(default)]
    pub jsonrpc: Option<JsonRpcConfig>,

    #[serde(default)]
    pub websocket: Option<WsConfig>,

    #[serde(default)]
    pub sse: Option<SseConfig>,

    #[serde(default)]
    pub auth_mode: AuthMode,

    #[serde(default)]
    pub embedded_auth: Option<AuthConfig>,
}

impl ServerConfig {
    #[must_use]
    pub const fn new(
        listen: Vec<Multiaddr>,
        admin: Option<AdminConfig>,
        jsonrpc: Option<JsonRpcConfig>,
        websocket: Option<WsConfig>,
        sse: Option<SseConfig>,
    ) -> Self {
        Self {
            listen,
            admin,
            jsonrpc,
            websocket,
            sse,
            auth_mode: AuthMode::Proxy,
            embedded_auth: None,
        }
    }

    #[must_use]
    pub const fn with_auth(
        listen: Vec<Multiaddr>,
        admin: Option<AdminConfig>,
        jsonrpc: Option<JsonRpcConfig>,
        websocket: Option<WsConfig>,
        sse: Option<SseConfig>,
        auth_mode: AuthMode,
        embedded_auth: Option<AuthConfig>,
    ) -> Self {
        Self {
            listen,
            admin,
            jsonrpc,
            websocket,
            sse,
            auth_mode,
            embedded_auth,
        }
    }

    #[must_use]
    pub fn embedded_auth(&self) -> Option<&AuthConfig> {
        self.embedded_auth.as_ref()
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct DataStoreConfig {
    pub path: Utf8PathBuf,
}

impl DataStoreConfig {
    #[must_use]
    pub const fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct BlobStoreConfig {
    pub path: Utf8PathBuf,
}

impl BlobStoreConfig {
    #[must_use]
    pub const fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}

impl ConfigFile {
    #[must_use]
    pub fn new(
        identity: IdentityConfig,
        mode: NodeMode,
        network: NetworkConfig,
        sync: SyncConfig,
        datastore: DataStoreConfig,
        blobstore: BlobStoreConfig,
        context: ContextConfig,
    ) -> Self {
        Self {
            identity,
            mode,
            network,
            sync,
            datastore,
            blobstore,
            context,
            tee: None,
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

        toml::from_str(&content).map_err(Into::into)
    }

    pub async fn save(&self, dir: &Utf8Path) -> EyreResult<()> {
        let path = dir.join(CONFIG_FILE);
        let content = toml::to_string_pretty(self)?;

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
}

pub mod serde_identity {
    use core::fmt::{self, Formatter};

    use serde::de::{self, MapAccess};
    use serde::ser::SerializeMap;
    use serde::{Deserializer, Serializer};

    use super::IdentityConfig;

    pub fn serialize<S>(identity: &IdentityConfig, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("peer_id", &identity.peer_id)?;
        if let Some(ref kp) = identity.keypair {
            map.serialize_entry("keypair", kp)?;
        }
        map.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<IdentityConfig, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IdentityVisitor;

        impl<'de> de::Visitor<'de> for IdentityVisitor {
            type Value = IdentityConfig;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                formatter.write_str("identity config with peer_id and optional keypair")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut peer_id = None::<String>;
                let mut keypair = None::<String>;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "peer_id" => peer_id = Some(map.next_value()?),
                        "keypair" => keypair = Some(map.next_value()?),
                        _ => {
                            drop(map.next_value::<de::IgnoredAny>());
                        }
                    }
                }

                let peer_id = peer_id.ok_or_else(|| de::Error::missing_field("peer_id"))?;

                Ok(IdentityConfig { peer_id, keypair })
            }
        }

        deserializer.deserialize_struct("IdentityConfig", &["peer_id", "keypair"], IdentityVisitor)
    }
}
