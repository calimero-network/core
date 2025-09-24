use std::collections::HashMap;
use std::sync::Arc;

use eyre::Result;
use serde_json::Value;

use crate::auth::token::TokenManager;
use crate::config::AuthConfig;
use crate::relayer::RelayerClient;
use crate::storage::{KeyManager, Storage};

// Export modules
pub mod core;
pub mod impls;

// Re-export core components
pub use core::provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};
pub use core::provider_registry::ProviderRegistration;

/// Provider context containing dependencies needed by providers
#[derive(Clone)]
pub struct ProviderContext {
    /// Storage backend
    pub storage: Arc<dyn Storage>,
    /// Key manager for domain operations
    pub key_manager: KeyManager,
    /// Token manager
    pub token_manager: TokenManager,
    /// Configuration
    pub config: Arc<AuthConfig>,
}

/// Provider factory for creating and registering authentication providers
pub struct ProviderFactory {
    registrations: HashMap<String, Arc<dyn ProviderRegistration>>,
}

impl ProviderFactory {
    /// Create a new provider factory with all registered providers
    pub fn new() -> Self {
        // Get all providers from the registry
        let registrations = core::provider_registry::get_all_provider_registrations()
            .into_iter()
            .map(|reg| (reg.provider_id().to_string(), reg))
            .collect();

        Self { registrations }
    }

    /// Create all enabled providers from configuration
    pub fn create_providers(
        &self,
        storage: Arc<dyn Storage>,
        config: &AuthConfig,
        token_manager: TokenManager,
    ) -> Result<Vec<Box<dyn AuthProvider>>, eyre::Error> {
        let mut providers = Vec::new();
        let key_manager = KeyManager::new(Arc::clone(&storage));
        let context = ProviderContext {
            storage,
            key_manager,
            token_manager: token_manager.clone(),
            config: Arc::new(config.clone()),
        };

        for registration in self.registrations.values() {
            if registration.is_enabled(config) {
                let mut provider = registration.create_provider(context.clone())?;
                
                // Configure relayer client for NEAR wallet provider if enabled
                if provider.name() == "near_wallet" && config.relayer.enabled {
                    let relayer_url = config.relayer.url.parse()
                        .map_err(|e| eyre::eyre!("Invalid relayer URL '{}': {}", config.relayer.url, e))?;
                    let relayer_client = RelayerClient::with_url(relayer_url);
                    
                    // Downcast to NearWalletProvider and add relayer client
                    if let Some(near_provider) = provider.as_any().downcast_ref::<impls::near_wallet::NearWalletProvider>() {
                        provider = Box::new(near_provider.clone().with_relayer_client(relayer_client));
                    }
                }
                
                providers.push(provider);
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
        token_manager: TokenManager,
    ) -> Result<Box<dyn AuthProvider>, eyre::Error> {
        if let Some(registration) = self.registrations.get(name) {
            let key_manager = KeyManager::new(Arc::clone(&storage));
            let context = ProviderContext {
                storage,
                key_manager,
                token_manager,
                config: Arc::new(config.clone()),
            };
            registration.create_provider(context)
        } else {
            Err(eyre::eyre!("Unknown provider: {}", name))
        }
    }

    /// Get information about available providers
    pub fn get_available_providers(&self, config: &AuthConfig) -> Vec<Value> {
        self.registrations
            .values()
            .map(|reg| {
                serde_json::json!({
                    "name": reg.provider_id(),
                    "available": true,
                    "enabled": reg.is_enabled(config),
                })
            })
            .collect()
    }
}

/// Create all enabled providers from configuration
pub fn create_providers(
    storage: Arc<dyn Storage>,
    config: &AuthConfig,
    token_manager: TokenManager,
) -> Result<Vec<Box<dyn AuthProvider>>, eyre::Error> {
    let factory = ProviderFactory::new();
    factory.create_providers(storage, config, token_manager)
}
