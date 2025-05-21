use std::{sync::Arc, time::{Duration, SystemTime, UNIX_EPOCH}};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use eyre::{eyre, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::storage::Storage;

const JWT_SECRET_KEY: &str = "system:jwt_secret";
const BACKUP_SECRET_KEY: &str = "system:jwt_secret_backup";

/// Secret rotation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRotationConfig {
    /// How often to rotate secrets (in seconds)
    pub rotation_interval: u64,
    /// How long to keep old secrets valid (in seconds)
    pub grace_period: u64,
}

impl Default for SecretRotationConfig {
    fn default() -> Self {
        Self {
            rotation_interval: 24 * 3600, // 24 hours
            grace_period: 48 * 3600,      // 48 hours
        }
    }
}

/// A versioned secret with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedSecret {
    /// The secret value (base64 encoded)
    pub value: String,
    /// Version identifier
    pub version: String,
    /// When this secret was created
    pub created_at: u64,
    /// When this secret expires
    pub expires_at: u64,
    /// Whether this is the primary secret
    pub is_primary: bool,
}

impl VersionedSecret {
    /// Create a new versioned secret
    pub fn new(rotation_config: &SecretRotationConfig) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Generate a secure random secret
        let mut secret = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut secret);
        
        Self {
            value: URL_SAFE_NO_PAD.encode(secret),
            version: format!("v{}", now),
            created_at: now,
            expires_at: now + rotation_config.grace_period,
            is_primary: true,
        }
    }

    /// Check if this secret has expired
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.expires_at < now
    }
}

/// Secret manager for handling the JWT secret
pub struct SecretManager {
    storage: Arc<dyn Storage>,
    secret: RwLock<Option<String>>,
}

impl SecretManager {
    /// Create a new secret manager
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self {
            storage,
            secret: RwLock::new(None),
        }
    }

    /// Initialize the secret manager
    pub async fn initialize(&self) -> Result<()> {
        // Try to load existing secret
        let secret = match self.storage.get(JWT_SECRET_KEY).await? {
            Some(data) => String::from_utf8(data)?,
            None => {
                // Generate new secret
                let new_secret = self.generate_secret();
                
                // Try to save to primary location
                if let Err(e) = self.storage.set(JWT_SECRET_KEY, new_secret.as_bytes()).await {
                    eprintln!("Failed to save secret to primary storage: {}", e);
                    
                    // Try backup location
                    self.storage.set(BACKUP_SECRET_KEY, new_secret.as_bytes()).await?;
                }
                
                new_secret
            }
        };

        *self.secret.write().await = Some(secret);
        Ok(())
    }

    /// Get the current JWT secret
    pub async fn get_secret(&self) -> Result<String> {
        if let Some(secret) = self.secret.read().await.as_ref() {
            Ok(secret.clone())
        } else {
            // This is a fallback in case the secret was somehow cleared from memory
            match self.storage.get(JWT_SECRET_KEY).await? {
                Some(data) => Ok(String::from_utf8(data)?),
                None => {
                    // Try backup location
                    match self.storage.get(BACKUP_SECRET_KEY).await? {
                        Some(data) => {
                            let secret = String::from_utf8(data)?;
                            // Restore to primary location
                            if let Err(e) = self.storage.set(JWT_SECRET_KEY, secret.as_bytes()).await {
                                eprintln!("Failed to restore secret to primary storage: {}", e);
                            }
                            Ok(secret)
                        }
                        None => Err(eyre!("No JWT secret found in storage")),
                    }
                }
            }
        }
    }

    /// Generate a new secure random secret
    fn generate_secret(&self) -> String {
        let mut secret = [0u8; 32]; // 256 bits
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut secret);
        URL_SAFE_NO_PAD.encode(secret)
    }
} 