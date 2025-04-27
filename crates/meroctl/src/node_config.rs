use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeConfig {
    pub aliases: BTreeMap<String, NodeConnection>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeConnection {
    Local { path: Utf8PathBuf },
    Remote { api: Url },
}

impl NodeConfig {
    pub fn load() -> Result<Self, std::io::Error> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&contents).unwrap())
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        let path = Self::config_path()?;
        let contents = toml::to_string(self).unwrap();
        std::fs::write(path, contents)
    }

    fn config_path() -> Result<Utf8PathBuf, std::io::Error> {
        let mut path = dirs::config_dir().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not find config directory",
            )
        })?;
        path.push("meroctl");
        path.push("nodes.toml");
        Ok(Utf8PathBuf::from_path_buf(path).unwrap())
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            aliases: BTreeMap::new(),
        }
    }
}
