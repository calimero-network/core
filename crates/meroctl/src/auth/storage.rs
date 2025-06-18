use async_trait::async_trait;
use eyre::{eyre, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

use super::tokens::AuthTokens;

/// Trait for secure token storage backends
#[async_trait]
pub trait SecureStorage: Send + Sync {
    async fn store_tokens(&self, profile: &str, tokens: &AuthTokens) -> Result<()>;
    async fn get_tokens(&self, profile: &str) -> Result<Option<AuthTokens>>;
    async fn delete_tokens(&self, profile: &str) -> Result<()>;
    async fn list_profiles(&self) -> Result<Vec<String>>;
}

/// Storage method preference
#[derive(Debug, Clone)]
pub enum TokenStorage {
    /// System keychain (most secure)
    Keychain,
    /// Encrypted file storage
    EncryptedFile,
    /// Environment variable (for CI/CD)
    Environment,
    /// Auto-detect best available method
    Auto,
}

/// Factory for creating storage backends
pub struct StorageFactory;

impl StorageFactory {
    /// Create the best available storage backend
    pub async fn create_storage(preferred: TokenStorage) -> Result<Box<dyn SecureStorage>> {
        match preferred {
            TokenStorage::Auto => Self::auto_detect().await,
            TokenStorage::Keychain => {
                if KeychainStorage::is_available() {
                    Ok(Box::new(KeychainStorage::new()?))
                } else {
                    // Fallback to encrypted file
                    Ok(Box::new(EncryptedFileStorage::new().await?))
                }
            }
            TokenStorage::EncryptedFile => Ok(Box::new(EncryptedFileStorage::new().await?)),
            TokenStorage::Environment => Ok(Box::new(EnvironmentStorage::new())),
        }
    }

    async fn auto_detect() -> Result<Box<dyn SecureStorage>> {
        // Try keychain first
        if KeychainStorage::is_available() {
            return Ok(Box::new(KeychainStorage::new()?));
        }

        // Fallback to encrypted file
        Ok(Box::new(EncryptedFileStorage::new().await?))
    }
}

/// Keychain-based storage (most secure)
pub struct KeychainStorage {
    service_name: String,
}

impl KeychainStorage {
    pub fn new() -> Result<Self> {
        Ok(Self {
            service_name: "meroctl".to_string(),
        })
    }

    pub fn is_available() -> bool {
        // For now, we'll implement a simple check
        // TODO: Add proper keychain availability detection
        cfg!(any(target_os = "macos", target_os = "windows", target_os = "linux"))
    }
}

#[async_trait]
impl SecureStorage for KeychainStorage {
    async fn store_tokens(&self, profile: &str, tokens: &AuthTokens) -> Result<()> {
        // TODO: Implement keychain storage using keyring crate
        // For now, fallback to encrypted file storage
        let file_storage = EncryptedFileStorage::new().await?;
        file_storage.store_tokens(profile, tokens).await
    }

    async fn get_tokens(&self, profile: &str) -> Result<Option<AuthTokens>> {
        // TODO: Implement keychain retrieval
        let file_storage = EncryptedFileStorage::new().await?;
        file_storage.get_tokens(profile).await
    }

    async fn delete_tokens(&self, profile: &str) -> Result<()> {
        // TODO: Implement keychain deletion
        let file_storage = EncryptedFileStorage::new().await?;
        file_storage.delete_tokens(profile).await
    }

    async fn list_profiles(&self) -> Result<Vec<String>> {
        // TODO: Implement keychain profile listing
        let file_storage = EncryptedFileStorage::new().await?;
        file_storage.list_profiles().await
    }
}

/// Encrypted file-based storage
pub struct EncryptedFileStorage {
    tokens_dir: PathBuf,
}

impl EncryptedFileStorage {
    pub async fn new() -> Result<Self> {
        let tokens_dir = Self::tokens_directory()?;
        fs::create_dir_all(&tokens_dir).await?;
        Ok(Self { tokens_dir })
    }

    fn tokens_directory() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| eyre!("Could not find config directory"))?;
        Ok(config_dir.join("meroctl").join("tokens"))
    }

    fn token_file_path(&self, profile: &str) -> PathBuf {
        self.tokens_dir.join(format!("{}.json", profile))
    }
}

#[async_trait]
impl SecureStorage for EncryptedFileStorage {
    async fn store_tokens(&self, profile: &str, tokens: &AuthTokens) -> Result<()> {
        let file_path = self.token_file_path(profile);
        let json = serde_json::to_string_pretty(tokens)?;
        
        // TODO: Add encryption here
        fs::write(file_path, json).await?;
        Ok(())
    }

    async fn get_tokens(&self, profile: &str) -> Result<Option<AuthTokens>> {
        let file_path = self.token_file_path(profile);
        
        if !file_path.exists() {
            return Ok(None);
        }

        let contents = fs::read_to_string(file_path).await?;
        // TODO: Add decryption here
        let tokens: AuthTokens = serde_json::from_str(&contents)?;
        Ok(Some(tokens))
    }

    async fn delete_tokens(&self, profile: &str) -> Result<()> {
        let file_path = self.token_file_path(profile);
        if file_path.exists() {
            fs::remove_file(file_path).await?;
        }
        Ok(())
    }

    async fn list_profiles(&self) -> Result<Vec<String>> {
        let mut profiles = Vec::new();
        let mut entries = fs::read_dir(&self.tokens_dir).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            if let Some(filename) = entry.file_name().to_str() {
                if filename.ends_with(".json") {
                    let profile = filename.trim_end_matches(".json");
                    profiles.push(profile.to_string());
                }
            }
        }
        
        Ok(profiles)
    }
}

/// Environment variable-based storage (for CI/CD)
pub struct EnvironmentStorage {
    cache: std::sync::Mutex<HashMap<String, AuthTokens>>,
}

impl EnvironmentStorage {
    pub fn new() -> Self {
        Self {
            cache: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl SecureStorage for EnvironmentStorage {
    async fn store_tokens(&self, profile: &str, tokens: &AuthTokens) -> Result<()> {
        let mut cache = self.cache.lock().unwrap();
        cache.insert(profile.to_string(), tokens.clone());
        Ok(())
    }

    async fn get_tokens(&self, profile: &str) -> Result<Option<AuthTokens>> {
        // Check environment variable first
        if let Ok(token) = std::env::var("MEROCTL_TOKEN") {
            if !token.is_empty() {
                // Create a minimal AuthTokens structure from env var
                return Ok(Some(AuthTokens::new(
                    profile.to_string(),
                    "http://localhost".parse().unwrap(),
                    token,
                    String::new(),
                    chrono::Utc::now() + chrono::Duration::hours(1),
                    vec![],
                )));
            }
        }

        // Check cache
        let cache = self.cache.lock().unwrap();
        Ok(cache.get(profile).cloned())
    }

    async fn delete_tokens(&self, profile: &str) -> Result<()> {
        let mut cache = self.cache.lock().unwrap();
        cache.remove(profile);
        Ok(())
    }

    async fn list_profiles(&self) -> Result<Vec<String>> {
        let cache = self.cache.lock().unwrap();
        Ok(cache.keys().cloned().collect())
    }
} 