use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use eyre::{eyre, Result};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeConfig {
    #[serde(rename = "nodes")]
    pub nodes: BTreeMap<String, NodeConnection>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NodeConnection {
    Local { path: Utf8PathBuf },
    Remote { url: Url },
}

impl NodeConfig {
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

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            nodes: BTreeMap::new(),
        }
    }
}
