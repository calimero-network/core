use std::fs::{read_to_string, write};

use calimero_network::config::{BootstrapConfig, CatchupConfig, DiscoveryConfig, SwarmConfig};
use calimero_server::admin::service::AdminConfig;
use calimero_server::jsonrpc::JsonRpcConfig;
use calimero_server::ws::WsConfig;
use camino::{Utf8Path, Utf8PathBuf};
use eyre::{Result as EyreResult, WrapErr};
use libp2p::{identity, Multiaddr};
use serde::{Deserialize, Serialize};
use url::Url;

const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Deserialize, Serialize)]
pub struct ConfigFile {
    #[serde(
        with = "calimero_primitives::identity::serde_identity",
        default = "identity::Keypair::generate_ed25519"
    )]
    pub identity: identity::Keypair,

    #[serde(flatten)]
    pub network: NetworkConfig,

    pub datastore: DataStoreConfig,

    pub blobstore: BlobStoreConfig,

    pub context: ContextConfig,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct NetworkConfig {
    pub swarm: SwarmConfig,

    pub server: ServerConfig,

    #[serde(default)]
    pub bootstrap: BootstrapConfig,

    #[serde(default)]
    pub discovery: DiscoveryConfig,

    pub catchup: CatchupConfig,
}

#[derive(Debug, Deserialize, Serialize)]
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
pub struct DataStoreConfig {
    pub path: Utf8PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BlobStoreConfig {
    pub path: Utf8PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ContextConfig {
    pub relayer: Url,
}

impl ConfigFile {
    pub(crate) fn exists(dir: &Utf8Path) -> bool {
        dir.join(CONFIG_FILE).is_file()
    }

    pub(crate) fn load(dir: &Utf8Path) -> EyreResult<Self> {
        let path = dir.join(CONFIG_FILE);
        let content = read_to_string(&path).wrap_err_with(|| {
            format!(
                "failed to read configuration from {:?}",
                dir.join(CONFIG_FILE)
            )
        })?;

        toml::from_str(&content).map_err(Into::into)
    }

    pub(crate) fn save(&self, dir: &Utf8Path) -> EyreResult<()> {
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
}
