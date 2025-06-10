use std::collections::BTreeMap;
use std::path::PathBuf;

use camino::Utf8PathBuf;
use eyre::{OptionExt, WrapErr};
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use tokio::fs;
use url::Url;

use crate::cli::ConnectionInfo;
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

impl Config {
    pub async fn load() -> eyre::Result<Self> {
        let path = Self::config_path()?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path).await?;

        let config = toml::from_str(&contents)?;

        Ok(config)
    }

    pub async fn save(&self) -> eyre::Result<()> {
        let path = Self::config_path()?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let contents = toml::to_string_pretty(self)?;

        fs::write(path, contents).await?;

        Ok(())
    }

    fn config_path() -> eyre::Result<PathBuf> {
        let config_dir = dirs::config_dir().ok_or_eyre("could not find config directory")?;

        Ok(config_dir.join("meroctl/nodes.toml"))
    }

    pub async fn get_connection(&self, node: &str) -> eyre::Result<Option<ConnectionInfo>> {
        let Some(connection) = self.nodes.get(node) else {
            return Ok(None);
        };

        let connection = match connection {
            NodeConnection::Local { path } => {
                let config = load_config(path, node).await?;
                let multiaddr = fetch_multiaddr(&config)?;
                let url = multiaddr_to_url(&multiaddr, "")?;

                ConnectionInfo::new(url, Some(config.identity)).await
            }
            NodeConnection::Remote { url, auth } => {
                let mut auth_key = None;

                if let Some(auth) = auth {
                    let bytes = bs58::decode(auth)
                        .into_vec()
                        .wrap_err("invalid base58 encoding")?;

                    let keypair = Keypair::from_protobuf_encoding(&bytes)
                        .wrap_err("invalid keypair encoding")?;

                    auth_key = Some(keypair);
                };

                ConnectionInfo::new(url.clone(), auth_key).await
            }
        };

        Ok(Some(connection))
    }
}
