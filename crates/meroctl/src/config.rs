use std::collections::BTreeMap;
use std::path::PathBuf;

use camino::Utf8PathBuf;
use eyre::{OptionExt, Result, WrapErr};
use serde::{Deserialize, Serialize};
use tokio::fs;
use url::Url;

use crate::common::{fetch_multiaddr, load_config, multiaddr_to_url};
use crate::connection::ConnectionInfo;
use crate::output::Output;
use crate::storage::{FileTokenStorage, JwtToken};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub nodes: BTreeMap<String, NodeConnection>,
    pub active_node: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NodeConnection {
    Local {
        path: Utf8PathBuf,
        jwt_tokens: Option<JwtToken>,
    },
    Remote {
        url: Url,
        jwt_tokens: Option<JwtToken>,
    }
}

impl Config {
    pub async fn load() -> Result<Self> {
        let path = Self::config_path().wrap_err("Failed to determine config path")?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path)
            .await
            .wrap_err_with(|| format!("Failed to read config file: {}", path.display()))?;

        let config = toml::from_str(&contents)
            .wrap_err_with(|| format!("Failed to parse config file: {}", path.display()))?;

        Ok(config)
    }

    pub async fn save(&self) -> Result<()> {
        let path = Self::config_path().wrap_err("Failed to determine config path")?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.wrap_err_with(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        let contents =
            toml::to_string_pretty(self).wrap_err("Failed to serialize config to TOML")?;

        fs::write(&path, contents)
            .await
            .wrap_err_with(|| format!("Failed to write config file: {}", path.display()))?;

        Ok(())
    }

    fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir().ok_or_eyre("could not find config directory")?;

        Ok(config_dir.join("calimero/meroctl/nodes.toml"))
    }

    pub async fn get_connection(
        &self,
        node: &str,
        output: Output,
    ) -> Result<Option<ConnectionInfo>> {
        let Some(connection) = self.nodes.get(node) else {
            return Ok(None);
        };

        let connection_info = match connection {
            NodeConnection::Local { path, jwt_tokens } => {
                let config = load_config(path, node)
                    .await
                    .wrap_err_with(|| format!("Failed to load config for local node '{}'", node))?;
                let multiaddr = fetch_multiaddr(&config).wrap_err_with(|| {
                    format!("Failed to fetch multiaddr for local node '{}'", node)
                })?;
                let url = multiaddr_to_url(&multiaddr, "").wrap_err_with(|| {
                    format!(
                        "Failed to convert multiaddr to URL for local node '{}'",
                        node
                    )
                })?;

                ConnectionInfo::new(
                    url, 
                    jwt_tokens.as_ref().map(|tokens| calimero_client::storage::JwtToken::with_refresh(
                        tokens.access_token.clone(),
                        tokens.refresh_token.clone().unwrap_or_default()
                    )), 
                    Some(node.to_owned()), 
                    crate::auth::create_cli_authenticator(output), 
                    FileTokenStorage::new()
                )
            }
            NodeConnection::Remote { url, jwt_tokens } => ConnectionInfo::new(
                url.clone(),
                jwt_tokens.as_ref().map(|tokens| calimero_client::storage::JwtToken::with_refresh(
                    tokens.access_token.clone(),
                    tokens.refresh_token.clone().unwrap_or_default()
                )),
                Some(node.to_owned()),
                crate::auth::create_cli_authenticator(output),
                FileTokenStorage::new(),
            ),
        };

        Ok(Some(connection_info))
    }
}




