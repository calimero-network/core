use async_trait::async_trait;
use eyre::Result as EyreResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use url::Url;

mod file;
mod keychain;
mod memory;

pub use file::FileStorage;
pub use keychain::KeychainStorage;
pub use memory::MemoryStorage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    pub node_url: Url,
    pub token: Option<JwtToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileTokens {
    pub profiles: HashMap<String, ProfileConfig>,
    pub current_profile: String,
}

#[async_trait]
pub trait TokenStorage: Send + Sync {
    async fn store_profile(&self, profile: &str, config: &ProfileConfig) -> EyreResult<()>;
    async fn load_profile(&self, profile: &str) -> EyreResult<Option<ProfileConfig>>;
    async fn remove_profile(&self, profile: &str) -> EyreResult<()>;
    async fn clear_all(&self) -> EyreResult<()>;
    async fn set_current_profile(&self, profile: &str) -> EyreResult<()>;
    // async fn get_current_profile(&self) -> EyreResult<Option<String>>;
    
    async fn list_profiles(&self) -> EyreResult<(Vec<String>, Option<String>)>;
    
    /// Get the current active profile and its config in one call to avoid multiple storage accesses
    async fn get_current_profile(&self) -> EyreResult<Option<(String, ProfileConfig)>>;
}

/// Create the appropriate storage backend based on system capabilities
pub fn create_storage() -> Box<dyn TokenStorage> {
    // Priority: Keychain -> File -> Memory (fallback)
    if KeychainStorage::is_available() {
        Box::new(KeychainStorage::new())
    } else {
        Box::new(FileStorage::new())
    }
}

/// Create in-memory storage for testing
pub fn create_memory_storage() -> Box<dyn TokenStorage> {
    Box::new(MemoryStorage::new())
} 