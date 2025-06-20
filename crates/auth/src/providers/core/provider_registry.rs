use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eyre::Result;
use lazy_static::lazy_static;

use crate::config::AuthConfig;
use crate::providers::core::provider::AuthProvider;
use crate::providers::ProviderContext;

/// Provider registration trait
///
/// This trait should be implemented by any provider that wants to be
/// automatically registered with the auth system.
pub trait ProviderRegistration: Send + Sync {
    /// Provider identifier
    fn provider_id(&self) -> &str;

    /// Create a provider instance
    fn create_provider(
        &self,
        context: ProviderContext,
    ) -> Result<Box<dyn AuthProvider>, eyre::Error>;

    /// Check if this provider is enabled in config
    fn is_enabled(&self, config: &AuthConfig) -> bool;
}

// A global registry for all providers
lazy_static! {
    static ref PROVIDER_REGISTRY: Mutex<ProviderRegistry> = Mutex::new(ProviderRegistry::new());
}

/// Global provider registry that collects all available auth providers
pub struct ProviderRegistry {
    registrations: HashMap<String, Arc<dyn ProviderRegistration>>,
}

impl ProviderRegistry {
    fn new() -> Self {
        Self {
            registrations: HashMap::new(),
        }
    }

    /// Register a provider implementation
    pub fn register(&mut self, registration: Arc<dyn ProviderRegistration>) {
        let id = registration.provider_id().to_string();
        self.registrations.insert(id, registration);
    }

    /// Get all registered providers
    pub fn get_all_registrations(&self) -> Vec<Arc<dyn ProviderRegistration>> {
        self.registrations.values().cloned().collect()
    }
}

/// Global function to register a provider
pub fn register_provider(registration: Arc<dyn ProviderRegistration>) {
    let mut registry = PROVIDER_REGISTRY.lock().unwrap();
    registry.register(registration);
}

/// Get all registered providers
pub fn get_all_provider_registrations() -> Vec<Arc<dyn ProviderRegistration>> {
    let registry = PROVIDER_REGISTRY.lock().unwrap();
    registry.get_all_registrations()
}
