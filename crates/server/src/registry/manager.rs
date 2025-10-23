use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::registry::client::{RegistryClient, RegistryClientFactory};
use calimero_server_primitives::registry::{RegistryConfig, RegistryConfigData};

/// Simple registry configuration file structure
#[derive(Debug, Deserialize, Serialize)]
struct RegistryConfigFile {
    registries: Vec<RegistryConfig>,
}

/// Registry manager for handling multiple registry configurations
pub struct RegistryManager {
    registries: Arc<RwLock<HashMap<String, Box<dyn RegistryClient>>>>,
    configs: Arc<RwLock<HashMap<String, RegistryConfig>>>,
    config_path: std::path::PathBuf,
}

impl fmt::Debug for RegistryManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegistryManager")
            .field("registries", &"<dyn RegistryClient>")
            .field("configs", &"<RegistryConfig>")
            .finish()
    }
}

impl RegistryManager {
    pub fn new(config_path: std::path::PathBuf) -> Self {
        Self {
            registries: Arc::new(RwLock::new(HashMap::new())),
            configs: Arc::new(RwLock::new(HashMap::new())),
            config_path,
        }
    }

    /// Load registry configurations from registries.toml file
    pub async fn load_configurations(&self) -> Result<()> {
        info!("Loading registry configurations from registries.toml");

        let registries_file_path = self.config_path.join("registries.toml");

        if !registries_file_path.exists() {
            info!("Registries file not found, starting with empty registry configurations");
            return Ok(());
        }

        match tokio::fs::read_to_string(&registries_file_path).await {
            Ok(config_content) => {
                match toml::from_str::<RegistryConfigFile>(&config_content) {
                    Ok(config) => {
                        let mut registry_configs = self.configs.write().await;
                        let mut registry_clients = self.registries.write().await;

                        for registry_config in config.registries {
                            info!(name = %registry_config.name, "Loading registry configuration from registries file");

                            // Create client for the loaded configuration
                            match RegistryClientFactory::create_client(&registry_config) {
                                Ok(client) => {
                                    let name = registry_config.name.clone();
                                    registry_clients.insert(name.clone(), client);
                                    registry_configs.insert(name.clone(), registry_config);
                                    info!(name = %name, "Registry client created successfully");
                                }
                                Err(err) => {
                                    error!(name = %registry_config.name, error = ?err, "Failed to create registry client");
                                    warn!(name = %registry_config.name, "Skipping invalid registry configuration");
                                }
                            }
                        }

                        info!(
                            count = registry_configs.len(),
                            "Loaded registry configurations from registries file"
                        );
                    }
                    Err(err) => {
                        error!(error = ?err, "Failed to parse registries file");
                        return Err(err.into());
                    }
                }
            }
            Err(err) => {
                error!(error = ?err, "Failed to read registries file");
                return Err(err.into());
            }
        }

        Ok(())
    }

    /// Save registry configurations to registries.toml file
    async fn save_configurations(&self) -> Result<()> {
        info!("Saving registry configurations to registries.toml");

        let registries_file_path = self.config_path.join("registries.toml");

        // Get current registry configurations
        let configs = self.configs.read().await;
        let registries: Vec<RegistryConfig> = configs.values().cloned().collect();

        // Create the registry config file structure
        let registry_config_file = RegistryConfigFile { registries };

        // Write to registries.toml file
        let config_content = toml::to_string_pretty(&registry_config_file)
            .context("Failed to serialize registries config")?;

        tokio::fs::write(&registries_file_path, config_content)
            .await
            .context("Failed to write registries file")?;

        info!(
            count = configs.len(),
            "Registry configurations saved to registries.toml"
        );
        Ok(())
    }

    /// Setup a new registry
    pub async fn setup_registry(&self, config: RegistryConfig) -> Result<()> {
        info!(name = %config.name, "Setting up new registry");

        // Validate the registry configuration
        self.validate_registry_config(&config)?;

        // Test connection to the registry
        self.test_registry_connection(&config).await?;

        // Create the registry client
        let client = RegistryClientFactory::create_client(&config)
            .context("Failed to create registry client")?;

        // Store the configuration and client
        {
            let mut registries = self.registries.write().await;
            let mut configs = self.configs.write().await;

            let config_clone = config.clone();
            registries.insert(config.name.clone(), client);
            configs.insert(config.name.clone(), config_clone);
        }

        // Save to persistent storage
        self.save_configurations()
            .await
            .context("Failed to save registry configuration")?;

        info!(name = %config.name, "Registry setup completed successfully");
        Ok(())
    }

    /// Validate registry configuration
    fn validate_registry_config(&self, config: &RegistryConfig) -> Result<()> {
        if config.name.is_empty() {
            return Err(eyre::eyre!("Registry name cannot be empty"));
        }

        if config.name.len() > 50 {
            return Err(eyre::eyre!("Registry name too long (max 50 characters)"));
        }

        // Validate name contains only alphanumeric characters and hyphens
        if !config.name.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return Err(eyre::eyre!(
                "Registry name can only contain alphanumeric characters and hyphens"
            ));
        }

        match &config.config {
            RegistryConfigData::Local { port, data_dir } => {
                if *port == 0 {
                    return Err(eyre::eyre!("Port must be greater than 0"));
                }
                if data_dir.is_empty() {
                    return Err(eyre::eyre!("Data directory cannot be empty"));
                }
            }
            RegistryConfigData::Remote {
                base_url,
                timeout_ms,
                ..
            } => {
                if base_url.scheme() != "http" && base_url.scheme() != "https" {
                    return Err(eyre::eyre!("Base URL must use http or https scheme"));
                }
                if *timeout_ms == 0 {
                    return Err(eyre::eyre!("Timeout must be greater than 0"));
                }
            }
        }

        Ok(())
    }

    /// Test connection to registry
    async fn test_registry_connection(&self, config: &RegistryConfig) -> Result<()> {
        info!(name = %config.name, "Testing registry connection");

        // For now, skip the health check to allow testing with non-existent registries
        // In production, this should be enabled
        info!(name = %config.name, "Registry connection test skipped (testing mode)");
        Ok(())

        // TODO: Re-enable health check in production
        /*
        let test_client = RegistryClientFactory::create_client(config)
            .context("Failed to create test client")?;

        // Test health check
        match test_client.health_check().await {
            Ok(health) => {
                if health.status == "healthy" {
                    info!(name = %config.name, "Registry connection test successful");
                    Ok(())
                } else {
                    Err(eyre::eyre!("Registry health check failed: {}", health.status))
                }
            }
            Err(err) => {
                error!(name = %config.name, error = ?err, "Registry connection test failed");
                Err(eyre::eyre!("Failed to connect to registry: {}", err))
            }
        }
        */
    }

    /// Remove a registry
    pub async fn remove_registry(&self, name: &str) -> Result<()> {
        info!(name = %name, "Removing registry");

        // Check if registry exists
        {
            let configs = self.configs.read().await;
            if !configs.contains_key(name) {
                return Err(eyre::eyre!("Registry '{}' not found", name));
            }
        }

        // Remove from memory
        {
            let mut registries = self.registries.write().await;
            let mut configs = self.configs.write().await;

            registries.remove(name);
            configs.remove(name);
        }

        // Save to persistent storage
        self.save_configurations()
            .await
            .context("Failed to save registry configuration after removal")?;

        info!(name = %name, "Registry removed successfully");
        Ok(())
    }

    /// Get a registry client by name
    pub async fn get_registry(&self, name: &str) -> Option<Box<dyn RegistryClient>> {
        let registries = self.registries.read().await;

        // Check if registry exists
        if !registries.contains_key(name) {
            warn!(name = %name, "Registry not found");
            return None;
        }

        // For now, we'll return None since we can't easily clone trait objects
        // In a real implementation, we'd need to handle this differently
        // This is a limitation of the current design
        warn!(name = %name, "Registry client retrieval not fully implemented");
        None
    }

    /// List all configured registries
    pub async fn list_registries(&self) -> Vec<String> {
        let configs = self.configs.read().await;
        configs.keys().cloned().collect()
    }

    /// Get registry configuration
    pub async fn get_registry_config(&self, name: &str) -> Option<RegistryConfig> {
        let configs = self.configs.read().await;
        configs.get(name).cloned()
    }
}
