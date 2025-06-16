use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use eyre::{eyre, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::storage::Storage;

// Storage keys for different types of secrets
const JWT_AUTH_SECRET_KEY: &str = "system:secrets:jwt_auth";
const JWT_CHALLENGE_SECRET_KEY: &str = "system:secrets:jwt_challenge";
const CSRF_SECRET_KEY: &str = "system:secrets:csrf";

// Backup keys
const JWT_AUTH_BACKUP_KEY: &str = "system:secrets:jwt_auth_backup";
const JWT_CHALLENGE_BACKUP_KEY: &str = "system:secrets:jwt_challenge_backup";
const CSRF_BACKUP_KEY: &str = "system:secrets:csrf_backup";

/// Secret type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretType {
    JwtAuth,
    JwtChallenge,
    Csrf,
}

impl SecretType {
    fn primary_key(&self) -> &'static str {
        match self {
            SecretType::JwtAuth => JWT_AUTH_SECRET_KEY,
            SecretType::JwtChallenge => JWT_CHALLENGE_SECRET_KEY,
            SecretType::Csrf => CSRF_SECRET_KEY,
        }
    }

    fn backup_key(&self) -> &'static str {
        match self {
            SecretType::JwtAuth => JWT_AUTH_BACKUP_KEY,
            SecretType::JwtChallenge => JWT_CHALLENGE_BACKUP_KEY,
            SecretType::Csrf => CSRF_BACKUP_KEY,
        }
    }
}

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
    /// The type of secret
    pub secret_type: SecretType,
}

impl VersionedSecret {
    /// Create a new versioned secret
    pub fn new(secret_type: SecretType, rotation_config: &SecretRotationConfig) -> Self {
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
            secret_type,
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

/// Secret manager for handling all system secrets
pub struct SecretManager {
    storage: Arc<dyn Storage>,
    secrets: RwLock<Vec<VersionedSecret>>,
    rotation_config: SecretRotationConfig,
}

impl SecretManager {
    /// Create a new secret manager
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self {
            storage,
            secrets: RwLock::new(Vec::new()),
            rotation_config: SecretRotationConfig::default(),
        }
    }

    /// Initialize the secret manager
    pub async fn initialize(&self) -> Result<()> {
        // Initialize all secret types
        for secret_type in [
            SecretType::JwtAuth,
            SecretType::JwtChallenge,
            SecretType::Csrf,
        ] {
            self.initialize_secret(secret_type).await?;
        }
        Ok(())
    }

    /// Initialize a specific secret type
    async fn initialize_secret(&self, secret_type: SecretType) -> Result<()> {
        let secret = match self.storage.get(secret_type.primary_key()).await? {
            Some(data) => serde_json::from_slice::<VersionedSecret>(&data)?,
            None => {
                // Generate new secret
                let new_secret = VersionedSecret::new(secret_type, &self.rotation_config);
                let data = serde_json::to_vec(&new_secret)?;

                // Try to save to primary location
                if let Err(e) = self.storage.set(secret_type.primary_key(), &data).await {
                    error!("Failed to save secret to primary storage: {}", e);

                    // Try backup location
                    self.storage.set(secret_type.backup_key(), &data).await?;
                }

                new_secret
            }
        };

        self.secrets.write().await.push(secret);
        Ok(())
    }

    /// Get a secret by type
    pub async fn get_secret(&self, secret_type: SecretType) -> Result<String> {
        // First try memory cache
        let secrets = self.secrets.read().await;
        if let Some(secret) = secrets
            .iter()
            .find(|s| s.secret_type == secret_type && s.is_primary)
        {
            return Ok(secret.value.clone());
        }
        drop(secrets);

        // If not in memory, try storage
        match self.storage.get(secret_type.primary_key()).await? {
            Some(data) => {
                let secret: VersionedSecret = serde_json::from_slice(&data)?;
                Ok(secret.value)
            }
            None => {
                // Try backup location
                match self.storage.get(secret_type.backup_key()).await? {
                    Some(data) => {
                        let secret: VersionedSecret = serde_json::from_slice(&data)?;
                        // Restore to primary location
                        if let Err(e) = self.storage.set(secret_type.primary_key(), &data).await {
                            error!("Failed to restore secret to primary storage: {}", e);
                        }
                        Ok(secret.value)
                    }
                    None => Err(eyre!("No secret found in storage for {:?}", secret_type)),
                }
            }
        }
    }

    /// Rotate a secret
    pub async fn rotate_secret(&self, secret_type: SecretType) -> Result<()> {
        let mut secrets = self.secrets.write().await;

        // Create new primary secret
        let new_secret = VersionedSecret::new(secret_type, &self.rotation_config);
        let data = serde_json::to_vec(&new_secret)?;

        // Save to storage
        self.storage.set(secret_type.primary_key(), &data).await?;

        // Update old secret as backup
        if let Some(old_secret) = secrets
            .iter_mut()
            .find(|s| s.secret_type == secret_type && s.is_primary)
        {
            old_secret.is_primary = false;
            let backup_data = serde_json::to_vec(old_secret)?;
            self.storage
                .set(secret_type.backup_key(), &backup_data)
                .await?;
        }

        // Update memory cache
        secrets.retain(|s| s.secret_type != secret_type || !s.is_expired());
        secrets.push(new_secret);

        info!("Rotated secret for {:?}", secret_type);
        Ok(())
    }

    /// Start the secret rotation task
    pub async fn start_rotation_task(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await; // Check every hour

                for secret_type in [
                    SecretType::JwtAuth,
                    SecretType::JwtChallenge,
                    SecretType::Csrf,
                ] {
                    if let Err(e) = self.rotate_if_needed(secret_type).await {
                        error!("Failed to rotate secret {:?}: {}", secret_type, e);
                    }
                }
            }
        });
    }

    /// Check and rotate a secret if needed
    async fn rotate_if_needed(&self, secret_type: SecretType) -> Result<()> {
        let secrets = self.secrets.read().await;
        if let Some(secret) = secrets
            .iter()
            .find(|s| s.secret_type == secret_type && s.is_primary)
        {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            if now - secret.created_at >= self.rotation_config.rotation_interval {
                drop(secrets);
                self.rotate_secret(secret_type).await?;
            }
        }
        Ok(())
    }

    /// Get the JWT auth secret
    pub async fn get_jwt_auth_secret(&self) -> Result<String> {
        self.get_secret(SecretType::JwtAuth).await
    }

    /// Get the JWT challenge secret
    pub async fn get_jwt_challenge_secret(&self) -> Result<String> {
        self.get_secret(SecretType::JwtChallenge).await
    }

    /// Get the CSRF secret
    pub async fn get_csrf_secret(&self) -> Result<String> {
        self.get_secret(SecretType::Csrf).await
    }
}
