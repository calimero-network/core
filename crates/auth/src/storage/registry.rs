use std::sync::{Arc, Mutex, Once};

use lazy_static::lazy_static;

use crate::config::StorageConfig;
use crate::storage::{KeyStorage, StorageError};

/// Storage provider trait
///
/// This trait is implemented by storage providers to register themselves
/// with the storage system.
pub trait StorageProvider: Send + Sync {
    /// Get the name of this storage provider
    fn name(&self) -> &str;

    /// Check if this provider supports the given configuration
    fn supports_config(&self, config: &StorageConfig) -> bool;

    /// Create a storage instance from the configuration
    fn create_storage(&self, config: &StorageConfig) -> Result<Arc<dyn KeyStorage>, StorageError>;
}

// Global registry for storage providers
lazy_static! {
    static ref STORAGE_REGISTRY: Mutex<StorageRegistry> = Mutex::new(StorageRegistry::new());
    static ref INIT: Once = Once::new();
}

/// Registry for storage providers
pub struct StorageRegistry {
    providers: Vec<Arc<dyn StorageProvider>>,
}

impl StorageRegistry {
    fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Register a storage provider
    pub fn register(&mut self, provider: Arc<dyn StorageProvider>) {
        self.providers.push(provider);
    }

    /// Get all registered providers
    pub fn get_providers(&self) -> Vec<Arc<dyn StorageProvider>> {
        self.providers.clone()
    }
}

/// Register a storage provider
pub fn register_provider(provider: Arc<dyn StorageProvider>) {
    INIT.call_once(|| {
        // Initialize any global state if needed
    });

    let mut registry = STORAGE_REGISTRY.lock().unwrap();
    registry.register(provider);
}

/// Get all registered storage providers
pub fn get_all_providers() -> Vec<Arc<dyn StorageProvider>> {
    let registry = STORAGE_REGISTRY.lock().unwrap();
    registry.get_providers()
}

/// Macro for registering storage providers
#[macro_export]
macro_rules! register_storage_provider {
    ($provider:expr) => {
        #[ctor::ctor]
        fn register_this_storage_provider() {
            use std::sync::Arc;
            $crate::storage::registry::register_provider(Arc::new($provider));
        }
    };
}
