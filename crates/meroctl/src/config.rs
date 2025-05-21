use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use eyre::{eyre, Result};
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::cli::{CliError, ConnectionInfo};
use crate::common::{fetch_multiaddr, load_config, multiaddr_to_url};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub nodes: BTreeMap<String, NodeConnection>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NodeConnection {
    Local { path: Utf8PathBuf },
    Remote { url: Url, auth: Option<String> },
}

impl NodeConnection {
    pub async fn get_connection_info(
        &self,
        node_name: Option<&str>,
    ) -> Result<ConnectionInfo, CliError> {
        match self {
            NodeConnection::Local { path } => {
                let config = load_config(path, node_name.unwrap_or_default()).await?;
                let multiaddr = fetch_multiaddr(&config)?;
                let url = multiaddr_to_url(&multiaddr, "")?;
                Ok(ConnectionInfo {
                    api_url: url,
                    auth_key: Some(config.identity),
                })
            }
            NodeConnection::Remote { url, auth } => {
                let auth_key = match auth {
                    Some(auth) => {
                        let bytes = bs58::decode(auth).into_vec().map_err(|e| {
                            CliError::Other(eyre!("Invalid base58 encoding: {}", e))
                        })?;
                        Some(Keypair::from_protobuf_encoding(&bytes).map_err(|e| {
                            CliError::Other(eyre!("Invalid keypair encoding: {}", e))
                        })?)
                    }
                    None => None,
                };

                Ok(ConnectionInfo {
                    api_url: url.clone(),
                    auth_key,
                })
            }
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)?;
        let config = toml::from_str(&contents)?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }

    fn config_path() -> Result<Utf8PathBuf> {
        let path = dirs::config_dir()
            .ok_or_else(|| eyre!("Could not find config directory"))?
            .join("meroctl/nodes.toml");
        Utf8PathBuf::from_path_buf(path).map_err(|_| eyre!("Failed to convert path to UTF-8"))
    }
}
