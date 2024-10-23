use std::fs::{read_to_string, write};

use calimero_context::config::ContextConfig;
use calimero_network::config::{BootstrapConfig, CatchupConfig, DiscoveryConfig, SwarmConfig};
use calimero_server::admin::service::AdminConfig;
use calimero_server::jsonrpc::JsonRpcConfig;
use calimero_server::ws::WsConfig;
use camino::{Utf8Path, Utf8PathBuf};
use eyre::{Result as EyreResult, WrapErr};
use libp2p::{identity, Multiaddr};
use serde::{Deserialize, Serialize};

const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
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
#[non_exhaustive]
pub struct NetworkConfig {
    pub swarm: SwarmConfig,

    pub server: ServerConfig,

    #[serde(default)]
    pub bootstrap: BootstrapConfig,

    #[serde(default)]
    pub discovery: DiscoveryConfig,

    pub catchup: CatchupConfig,
}

impl NetworkConfig {
    pub fn new(
        swarm: SwarmConfig,
        bootstrap: BootstrapConfig,
        discovery: DiscoveryConfig,
        server: ServerConfig,
        catchup: CatchupConfig,
    ) -> Self {
        Self {
            swarm,
            server,
            bootstrap,
            discovery,
            catchup,
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
    pub fn new(
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
    pub fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct BlobStoreConfig {
    pub path: Utf8PathBuf,
}

impl BlobStoreConfig {
    pub fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}

impl ConfigFile {
    pub fn new(
        identity: identity::Keypair,
        datastore: DataStoreConfig,
        blobstore: BlobStoreConfig,
        context: ContextConfig,
        network: NetworkConfig,
    ) -> Self {
        Self {
            identity,
            datastore,
            blobstore,
            context,
            network,
        }
    }

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
}
