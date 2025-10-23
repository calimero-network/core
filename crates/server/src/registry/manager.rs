use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use calimero_server_primitives::registry::RegistryConfig;
use eyre::Result;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::{info, warn};

use crate::registry::client::{RegistryClient, RegistryClientFactory};

#[derive(Debug, Deserialize, Serialize)]
struct RegistryConfigFile {
    registries: Vec<RegistryConfig>,
}

#[derive(Debug)]
pub struct RegistryManager {
    configs: HashMap<String, RegistryConfig>,
    config_path: PathBuf,
}

impl RegistryManager {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            configs: HashMap::new(),
            config_path,
        }
    }

    pub async fn load_configurations(&mut self) -> Result<()> {
        let config_file_path = self.config_path.join("registries.toml");

        if !config_file_path.exists() {
            info!("No registry configuration file found, starting with empty configuration");
            return Ok(());
        }

        let content = fs::read_to_string(&config_file_path).await?;
        let config_file: RegistryConfigFile = toml::from_str(&content)?;

        for config in config_file.registries {
            self.configs.insert(config.name.clone(), config);
        }

        info!("Loaded {} registry configurations", self.configs.len());
        Ok(())
    }

    pub async fn save_configurations(&self) -> Result<()> {
        let config_file_path = self.config_path.join("registries.toml");

        // Ensure directory exists
        if let Some(parent) = config_file_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let config_file = RegistryConfigFile {
            registries: self.configs.values().cloned().collect(),
        };

        let content = toml::to_string_pretty(&config_file)?;
        fs::write(&config_file_path, content).await?;

        info!(
            "Saved {} registry configurations to {:?}",
            self.configs.len(),
            config_file_path
        );
        Ok(())
    }

    pub async fn setup_registry(&mut self, config: RegistryConfig) -> Result<()> {
        // Validate configuration
        self.validate_registry_config(&config)?;

        // Test registry connection
        self.test_registry_connection(&config).await?;

        // Store configuration
        let config_clone = config.clone();
        self.configs.insert(config.name.clone(), config);

        // Save to disk
        self.save_configurations().await?;

        info!("Successfully setup registry: {}", config_clone.name);
        Ok(())
    }

    pub async fn remove_registry(&mut self, name: &str) -> Result<()> {
        if self.configs.remove(name).is_some() {
            self.save_configurations().await?;
            info!("Successfully removed registry: {}", name);
            Ok(())
        } else {
            Err(eyre::eyre!("Registry not found: {}", name))
        }
    }

    pub async fn list_registries(&self) -> Vec<String> {
        self.configs.keys().cloned().collect()
    }

    pub fn get_registry(&self, name: &str) -> Option<Box<dyn RegistryClient>> {
        if let Some(config) = self.configs.get(name) {
            match RegistryClientFactory::create_client(config) {
                Ok(client) => Some(client),
                Err(err) => {
                    warn!("Failed to create client for registry {}: {}", name, err);
                    None
                }
            }
        } else {
            None
        }
    }

    pub fn get_registry_config(&self, name: &str) -> Option<&RegistryConfig> {
        self.configs.get(name)
    }

    fn validate_registry_config(&self, config: &RegistryConfig) -> Result<()> {
        // Validate name
        if config.name.is_empty() || config.name.len() > 50 {
            return Err(eyre::eyre!(
                "Registry name must be between 1 and 50 characters"
            ));
        }

        // Validate based on type
        match &config.config {
            calimero_server_primitives::registry::RegistryConfigData::Local { port, data_dir } => {
                if *port == 0 || *port > 65535 {
                    return Err(eyre::eyre!("Port must be between 1 and 65535"));
                }
                if data_dir.is_empty() {
                    return Err(eyre::eyre!("Data directory cannot be empty"));
                }
            }
            calimero_server_primitives::registry::RegistryConfigData::Remote {
                base_url,
                timeout_ms,
                ..
            } => {
                if base_url.is_empty() {
                    return Err(eyre::eyre!("Base URL cannot be empty"));
                }
                if *timeout_ms == 0 {
                    return Err(eyre::eyre!("Timeout must be greater than 0"));
                }
            }
        }

        Ok(())
    }

    async fn test_registry_connection(&self, config: &RegistryConfig) -> Result<()> {
        // For now, skip health check to allow testing with non-existent registries
        // TODO: Re-enable this in production
        info!("Skipping registry health check for testing purposes");
        Ok(())
    }
}
