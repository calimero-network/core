use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use eyre::Result as EyreResult;
use serde::{Deserialize, Serialize};
use url::Url;

mod file;
mod keychain;

pub use file::FileStorage;
pub use keychain::KeychainStorage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    pub auth_profile: String,
    pub node_url: Url,
    pub token: Option<JwtToken>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AllProfiles {
    pub profiles: HashMap<String, ProfileConfig>,
    pub active_profile: Option<String>,
}

#[async_trait]
pub trait TokenStorage: Send + Sync {
    /// Load all profiles in a single operation
    async fn load_all_profiles(&self) -> EyreResult<AllProfiles>;

    /// Save all profiles in a single operation
    async fn save_all_profiles(&self, profiles: &AllProfiles) -> EyreResult<()>;

    /// Get a specific profile config
    async fn load_profile(&self, name: &str) -> EyreResult<Option<ProfileConfig>> {
        Ok(self.load_all_profiles().await?.profiles.get(name).cloned())
    }

    /// Store a profile config
    async fn store_profile(&self, name: &str, config: &ProfileConfig) -> EyreResult<()>;

    /// Remove a profile
    async fn remove_profile(&self, name: &str) -> EyreResult<()>;

    /// Get current active profile with its config
    async fn get_current_profile(&self) -> EyreResult<Option<(String, ProfileConfig)>>;

    /// Set active profile
    async fn set_current_profile(&self, name: &str) -> EyreResult<()>;

    /// List all profiles
    async fn list_profiles(&self) -> EyreResult<(Vec<String>, Option<String>)>;

    /// Clear all profiles
    async fn clear_all(&self) -> EyreResult<()>;
}

/// Global storage instance to maximize cache utilization
/// This ensures we use the same storage instance across the entire application lifecycle
static GLOBAL_STORAGE: OnceLock<Arc<dyn TokenStorage>> = OnceLock::new();

/// Get the global storage instance - reuses the same instance for maximum cache efficiency
pub fn get_storage() -> &'static Arc<dyn TokenStorage> {
    GLOBAL_STORAGE.get_or_init(|| {
        let storage: Arc<dyn TokenStorage> = if KeychainStorage::is_available() {
            Arc::new(KeychainStorage::new())
        } else {
            Arc::new(FileStorage::new())
        };
        storage
    })
}
