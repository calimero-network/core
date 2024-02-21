use std::fs;
use std::net::SocketAddr;

use color_eyre::eyre::{self, Context};
use serde::{Deserialize, Serialize};

const CONFIG_FILE: &str = "config.toml";
pub const DEFAULT_API_HOST: &str = "127.0.0.1";
pub const DEFAULT_API_PORT: u16 = 3030;
pub const DEFAULT_CALIMERO_CHAT_HOME: &str = ".calimero/peer";

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub websocket_api: WsApiConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WsApiConfig {
    pub host: String,
    pub port: u16,
}

impl Default for WsApiConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_API_HOST.to_string(),
            port: DEFAULT_API_PORT,
        }
    }
}

impl WsApiConfig {
    pub fn get_socket_addr(&self) -> eyre::Result<SocketAddr> {
        Ok(format!("{}:{}", self.host, self.port).parse::<SocketAddr>()?)
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
