use std::collections::HashMap;
use std::sync::Arc;

use eyre::Result;
use serde_json::Value;

use crate::auth::token::TokenManager;
use crate::config::AuthConfig;
use crate::storage::Storage;

pub mod near_wallet;
pub mod provider;

// Re-export AuthProvider and related types from the provider module
pub use provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};

/// Provider factory for creating and registering authentication providers
pub struct ProviderFactory {
    providers: HashMap<
        String,
        Box<
            dyn Fn(Arc<dyn Storage>, &AuthConfig) -> Result<Box<dyn AuthProvider>, eyre::Error>
                + Send
                + Sync,
        >,
    >,
}

impl ProviderFactory {
    /// Create a new provider factory with default providers
    pub fn new() -> Self {
        let mut factory = Self {
            providers: HashMap::new(),
        };

        // Register the default providers
        factory.register_near_wallet();

        factory
    }

    /// Register the NEAR wallet provider
    pub fn register_near_wallet(&mut self) {
        self.register("near_wallet", |storage, config| {
            let near_config = config.near.clone();
            let token_manager = TokenManager::new(config.jwt.clone(), storage.clone());
            let provider =
                near_wallet::NearWalletProvider::new(near_config, storage, token_manager);
            Ok(Box::new(provider))
        });
    }

    /// Register a provider factory function
    pub fn register<F>(&mut self, name: &str, factory: F)
    where
        F: Fn(Arc<dyn Storage>, &AuthConfig) -> Result<Box<dyn AuthProvider>, eyre::Error>
            + Send
            + Sync
            + 'static,
    {
        self.providers.insert(name.to_string(), Box::new(factory));
    }

    /// Create all enabled providers from configuration
    pub fn create_providers(
        &self,
        storage: Arc<dyn Storage>,
        config: &AuthConfig,
    ) -> Result<Vec<Box<dyn AuthProvider>>, eyre::Error> {
        let mut providers = Vec::new();

        for provider_name in &config.enabled_providers {
            if let Some(factory) = self.providers.get(provider_name) {
                let provider = factory(storage.clone(), config)?;
                providers.push(provider);
            } else {
                return Err(eyre::eyre!("Unknown provider: {}", provider_name));
            }
        }

        Ok(providers)
    }

    /// Create a provider by name
    pub fn create_provider(
        &self,
        name: &str,
        storage: Arc<dyn Storage>,
        config: &AuthConfig,
    ) -> Result<Box<dyn AuthProvider>, eyre::Error> {
        if let Some(factory) = self.providers.get(name) {
            factory(storage, config)
        } else {
            Err(eyre::eyre!("Unknown provider: {}", name))
        }
    }

    /// Get information about available providers
    pub fn get_available_providers(&self) -> Vec<Value> {
        self.providers
            .keys()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "available": true,
                })
            })
            .collect()
    }
}

/// Create all enabled providers from configuration
pub fn create_providers(
    storage: Arc<dyn Storage>,
    config: &AuthConfig,
) -> Result<Vec<Box<dyn AuthProvider>>, eyre::Error> {
    let factory = ProviderFactory::new();
    factory.create_providers(storage, config)
}
