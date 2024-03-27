use std::fs;

use eyre::WrapErr;
use libp2p::{identity, Multiaddr};
use serde::{Deserialize, Serialize};

const CONFIG_FILE: &str = "config.toml";

pub const DEFAULT_CALIMERO_CHAT_HOME: &str = ".calimero/experiments/chat-p0c";

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(
        with = "calimero_identity::config::serde_identity",
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

    pub server: ServerConfig,

    #[serde(default)]
    pub bootstrap: calimero_network::config::BootstrapConfig,

    #[serde(default)]
    pub discovery: calimero_network::config::DiscoveryConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub listen: Vec<Multiaddr>,

    #[serde(default)]
    pub admin: Option<calimero_server::admin::AdminConfig>,

    #[serde(default)]
    pub graphql: Option<calimero_server::graphql::GraphQLConfig>,

    #[serde(default)]
    pub websocket: Option<calimero_server::websocket::WsConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StoreConfig {
    pub path: camino::Utf8PathBuf,
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
