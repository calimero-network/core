use core::time::Duration;
use std::collections::BTreeMap;
use std::fs::{read_to_string, write};

use calimero_context::config::ContextConfig;
use calimero_network::config::{BootstrapConfig, DiscoveryConfig, SwarmConfig};
use calimero_server::admin::service::AdminConfig;
use calimero_server::jsonrpc::JsonRpcConfig;
use calimero_server::ws::WsConfig;
use camino::{Utf8Path, Utf8PathBuf};
use eyre::{Result as EyreResult, WrapErr};
use libp2p_identity::Keypair;
use multiaddr::Multiaddr;
use serde::{Deserialize, Serialize};
use toml_edit::{Document, Item, Table, Value};

pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ConfigFile {
    #[serde(
        with = "serde_identity",
        default = "libp2p_identity::Keypair::generate_ed25519"
    )]
    pub identity: Keypair,

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

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct DataStoreConfig {
    pub path: Utf8PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct BlobStoreConfig {
    pub path: Utf8PathBuf,
}

impl ConfigFile {
    pub fn exists(dir: &Utf8Path) -> bool {
        dir.join(CONFIG_FILE).is_file()
    }

    pub fn load(dir: &Utf8Path) -> EyreResult<Self> {
        let content = read_to_string(dir.join(CONFIG_FILE)).wrap_err_with(|| {
            format!("failed to read configuration from {:?}", dir.join(CONFIG_FILE))
        })?;
        toml::from_str(&content).map_err(Into::into)
    }

    pub fn save(&self, dir: &Utf8Path) -> EyreResult<()> {
        let path = dir.join(CONFIG_FILE);
        let content = toml::to_string_pretty(self)?;
        write(&path, content).wrap_err_with(|| {
            format!("failed to write configuration to {:?}", dir.join(CONFIG_FILE))
        })?;
        Ok(())
    }

    pub fn as_map(&self) -> toml::Value {
        toml::Value::try_from(self).unwrap_or_else(|_| toml::Value::Table(Default::default()))
    }

    pub fn view_keys(&self, keys: &[String]) -> EyreResult<toml::Value> {
        let full = self.as_map();
        let mut subset = toml::value::Table::new();

        for key in keys {
            if let Some(value) = get_nested_item(&full, key) {
                insert_nested_key(&mut subset, key, value.clone());
            }
        }

        Ok(toml::Value::Table(subset))
    }

    pub fn apply_edits(&self, edits: &BTreeMap<String, String>) -> EyreResult<(toml_edit::Table, toml_edit::Table)> {
        let original_doc = self.to_document()?;
        let mut updated_doc = original_doc.clone();
        let mut diff = Table::new();

        for (key, value_str) in edits {
            let parsed = parse_toml_value(value_str)?;
            let keys: Vec<&str> = key.split('.').collect();
            let updated = set_nested_item(updated_doc.as_table_mut(), &keys, parsed.clone());

            if updated {
                set_nested_item(diff.as_table_mut(), &keys, parsed);
            }
        }

        Ok((diff, updated_doc.as_table().clone()))
    }

    fn to_document(&self) -> EyreResult<Document> {
        let toml_str = toml::to_string_pretty(self)?;
        let doc = toml_str.parse::<Document>()?;
        Ok(doc)
    }
}

fn parse_toml_value(s: &str) -> EyreResult<Item> {
    let wrapped = format!("val = {}", s);
    let parsed: Document = wrapped.parse()?;
    Ok(parsed["val"].clone())
}

fn get_nested_item<'a>(root: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    key.split('.').fold(Some(root), |acc, k| acc?.get(k))
}

fn insert_nested_key<'a>(table: &'a mut toml::value::Table, key: &str, val: toml::Value) {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = table;

    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            current.insert(part.to_string(), val);
        } else {
            current = current
                .entry(part.to_string())
                .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
                .as_table_mut()
                .unwrap();
        }
    }
}

fn set_nested_item(root: &mut Table, keys: &[&str], val: Item) -> bool {
    let mut current = root;

    for (i, k) in keys.iter().enumerate() {
        if i == keys.len() - 1 {
            current.insert(k, val);
            return true;
        } else {
            current = current
                .entry(k)
                .or_insert(Item::Table(Table::new()))
                .as_table_mut()
                .unwrap();
        }
    }

    false
}

impl DataStoreConfig {
    #[must_use]
    pub const fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}

impl BlobStoreConfig {
    #[must_use]
    pub const fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
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

mod serde_duration {
    use core::time::Duration;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        serializer.serialize_u64(duration.as_millis() as u64)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where D: Deserializer<'de> {
        u64::deserialize(deserializer).map(Duration::from_millis)
    }
}

pub mod serde_identity {
    use core::fmt::{self, Formatter};
    use libp2p_identity::Keypair;
    use serde::de::{self, MapAccess};
    use serde::ser::{SerializeMap, Serializer};
    use serde::{Deserializer};

    pub fn serialize<S>(key: &Keypair, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        let mut keypair = serializer.serialize_map(Some(2))?;
        keypair.serialize_entry("peer_id", &key.public().to_peer_id().to_base58())?;
        keypair.serialize_entry(
            "keypair",
            &bs58::encode(&key.to_protobuf_encoding().map_err(serde::ser::Error::custom)?).into_string(),
        )?;
        keypair.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Keypair, D::Error>
    where D: Deserializer<'de> {
        struct IdentityVisitor;

        impl<'de> de::Visitor<'de> for IdentityVisitor {
            type Value = Keypair;

            fn expecting(&self, f: &mut Formatter<'_>) -> fmt::Result {
                f.write_str("an identity")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where A: MapAccess<'de> {
                let mut peer_id = None::<String>;
                let mut priv_key = None::<String>;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "peer_id" => peer_id = Some(map.next_value()?),
                        "keypair" => priv_key = Some(map.next_value()?),
                        _ => { let _ = map.next_value::<de::IgnoredAny>(); }
                    }
                }

                let peer_id = peer_id.ok_or_else(|| de::Error::missing_field("peer_id"))?;
                let priv_key = priv_key.ok_or_else(|| de::Error::missing_field("keypair"))?;

                let decoded = bs58::decode(priv_key)
                    .into_vec()
                    .map_err(|_| de::Error::custom("invalid base58"))?;

                let keypair = Keypair::from_protobuf_encoding(&decoded)
                    .map_err(|_| de::Error::custom("invalid protobuf"))?;

                if peer_id != keypair.public().to_peer_id().to_base58() {
                    return Err(de::Error::custom("Peer ID does not match public key"));
                }

                Ok(keypair)
            }
        }

        deserializer.deserialize_struct("Keypair", &["peer_id", "keypair"], IdentityVisitor)
    }
}
