// crates/config/src/lib.rs

use clap::ValueEnum;
use core::time::Duration;
use std::collections::HashMap;
use std::fs::{read_to_string, write};

use calimero_context::config::ContextConfig;
use calimero_network::config::{BootstrapConfig, DiscoveryConfig, SwarmConfig};
use calimero_server::admin::service::AdminConfig;
use calimero_server::jsonrpc::JsonRpcConfig;
use calimero_server::ws::WsConfig;
use camino::{Utf8Path, Utf8PathBuf};
use eyre::{Result as EyreResult, WrapErr};
use multiaddr::Multiaddr;
use serde::{Deserialize, Serialize};
use serde_json;

pub const CONFIG_FILE: &str = "config.toml";

pub mod configHints;

#[derive(Debug, Clone, Copy, ValueEnum)]  // Add ValueEnum here
#[clap(rename_all = "lower")]
pub enum OutputFormat {
    Json,
    Pretty,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ConfigFile {
    #[serde(
        with = "serde_identity",
        default = "libp2p_identity::Keypair::generate_ed25519"
    )]
    pub identity: libp2p_identity::Keypair,

    #[serde(flatten)]
    pub network: NetworkConfig,

    pub sync: SyncConfig,

    pub datastore: DataStoreConfig,

    pub blobstore: BlobStoreConfig,

    pub context: ContextConfig,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct SyncConfig {
    #[serde(rename = "timeout_ms", with = "serde_duration")]
    pub timeout: Duration,
    #[serde(rename = "interval_ms", with = "serde_duration")]
    pub interval: Duration,
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
}

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
        }
    }

    #[must_use]
    pub fn exists(dir: &Utf8Path) -> bool {
        dir.join(CONFIG_FILE).is_file()
    }

    pub fn load(dir: &Utf8Path) -> EyreResult<Self> {
        let path = dir.join(CONFIG_FILE);
        let content = read_to_string(&path).wrap_err_with(|| {
            format!(
                "failed to read configuration from {:?}",
                dir.join(CONFIG_FILE)
            )
        })?;

        toml::from_str(&content).map_err(Into::into)
    }

    pub fn save(&self, dir: &Utf8Path) -> EyreResult<()> {
        let path = dir.join(CONFIG_FILE);
        let content = toml::to_string_pretty(self)?;

        write(&path, content).wrap_err_with(|| {
            format!(
                "failed to write configuration to {:?}",
                dir.join(CONFIG_FILE)
            )
        })?;

        Ok(())
    }

    /// Only write config file if changes are detected
    pub fn save_if_changed(&self, dir: &Utf8Path) -> EyreResult<bool> {
        let path = dir.join(CONFIG_FILE);
        let new_content = toml::to_string_pretty(self)?;

        let changed = match read_to_string(&path) {
            Ok(existing) => existing != new_content,
            Err(_) => true,
        };

        if changed {
            write(&path, new_content).wrap_err_with(|| {
                format!("failed to write configuration to {:?}", path)
            })?;
        }

        Ok(changed)
    }

    /// Print config to stdout
    pub fn print(&self, format: OutputFormat) -> EyreResult<()> {
        match format {
            OutputFormat::Pretty => {
                println!("{:#?}", self);
            }
            OutputFormat::Json => {
                let json = serde_json::to_string_pretty(self)?;
                println!("{}", json);
            }
        }
        Ok(())
    }

    /// Provide editable keys with example values
    pub fn editable_keys() -> HashMap<&'static str, Vec<&'static str>> {
        let mut map = HashMap::new();

        map.insert("sync.timeout_ms", vec!["100", "1000", "5000"]);
        map.insert("sync.interval_ms", vec!["100", "1000", "5000"]);
        map.insert("network.bootstrap.nodes", vec!["multiaddr strings"]);
        map.insert("datastore.path", vec!["/path/to/data"]);
        map.insert("blobstore.path", vec!["/path/to/blob"]);

        map
    }

    pub fn print_hints() {
        let map = Self::editable_keys();
        println!("Editable config keys and example values:");
        for (key, values) in map {
            println!("  {}: {:?}", key, values);
        }
    }

    /// Get the value for a specific config key
    pub fn get_value(&self, key: &str) -> Option<String> {
        match key {
            "sync.timeout_ms" => Some(self.sync.timeout.as_millis().to_string()),
            "sync.interval_ms" => Some(self.sync.interval.as_millis().to_string()),
            "network.swarm.port" => Some(self.network.swarm.port.to_string()),
            "network.server.listen" => Some(
                self.network
                    .server
                    .listen
                    .iter()
                    .map(|addr| addr.to_string())
                    .collect::<Vec<String>>()
                    .join(", "),
            ),
            _ => None,
        }
    }

    /// Print the hint for a specific key
    pub fn print_hint_for_key(key: &str) {
        if let Some(hint) = configHints::CONFIG_HINTS.iter().find(|h| h.key == key) {
            println!("Key: {}", hint.key);
            println!("Description: {}", hint.description);
        } else {
            println!("No hint available for key: {}", key);
        }
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
                        "keypair" => priv_key = Some(map.next_value()?
),
_ => {}
}
}


                        let peer_id = peer_id.ok_or_else(|| de::Error::missing_field("peer_id"))?;
            let priv_key = priv_key.ok_or_else(|| de::Error::missing_field("keypair"))?;

            let decoded_priv_key = bs58::decode(&priv_key).into_vec().map_err(de::Error::custom)?;

            Keypair::from_protobuf_encoding(&decoded_priv_key).map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_map(IdentityVisitor)
}
