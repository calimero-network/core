use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use eyre::{eyre, Result};
use rand::Rng;
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

/// Current Unix time in whole seconds.
///
/// Returns an error instead of panicking when the system clock is set before
/// the Unix epoch (1970). This keeps every time-dependent path panic-free
/// (security finding #12).
fn current_unix_secs() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| eyre!("system clock is set before the Unix epoch: {e}"))?
        .as_secs())
}

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
    pub fn new(secret_type: SecretType, rotation_config: &SecretRotationConfig) -> Result<Self> {
        let now = current_unix_secs()?;

        // Generate a secure random secret
        let secret: [u8; 32] = rand::thread_rng().gen();

        Ok(Self {
            value: URL_SAFE_NO_PAD.encode(secret),
            version: format!("v{now}"),
            created_at: now,
            expires_at: now + rotation_config.grace_period,
            is_primary: true,
            secret_type,
        })
    }

    /// Check if this secret has expired
    pub fn is_expired(&self) -> Result<bool> {
        Ok(self.expires_at < current_unix_secs()?)
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
                let new_secret = VersionedSecret::new(secret_type, &self.rotation_config)?;
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

    /// Get the current primary secret value for a type.
    ///
    /// Backing storage is treated as authoritative: it is read on every call so
    /// that a rotation performed by another process (which only mutates storage,
    /// not this process's in-memory cache) is observed. When the stored
    /// `version` differs from the cached one, the in-memory cache is refreshed
    /// before returning. This is the staleness half of finding #12 (Fix C):
    /// without it, replicas keep serving a secret that has already rotated
    /// elsewhere and diverge.
    pub async fn get_secret(&self, secret_type: SecretType) -> Result<String> {
        // Storage is authoritative for the current primary.
        if let Some(data) = self.storage.get(secret_type.primary_key()).await? {
            let stored: VersionedSecret = serde_json::from_slice(&data)?;
            self.refresh_cache_primary(secret_type, &stored).await;
            return Ok(stored.value);
        }

        // Primary missing in storage: fall back to the in-memory cache, if any.
        {
            let secrets = self.secrets.read().await;
            if let Some(secret) = secrets
                .iter()
                .find(|s| s.secret_type == secret_type && s.is_primary)
            {
                return Ok(secret.value.clone());
            }
        }

        // Last resort: recover from the backup location and restore it.
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

    /// Refresh the cached primary for a type when the stored version differs.
    ///
    /// Keeps the in-memory cache consistent with backing storage after an
    /// out-of-process rotation (Fix C). The previous primary is demoted to a
    /// non-primary cache entry so it can still serve as a verification fallback
    /// until it expires, mirroring [`Self::rotate_secret`].
    async fn refresh_cache_primary(&self, secret_type: SecretType, stored: &VersionedSecret) {
        let mut secrets = self.secrets.write().await;

        if let Some(cached) = secrets
            .iter()
            .find(|s| s.secret_type == secret_type && s.is_primary)
        {
            if cached.version == stored.version {
                return; // Cache already up to date.
            }
        }

        // Demote any stale primary to a backup so it remains a verify fallback.
        for s in secrets
            .iter_mut()
            .filter(|s| s.secret_type == secret_type && s.is_primary)
        {
            s.is_primary = false;
        }

        // Drop expired entries and any duplicate of the incoming version.
        let now = current_unix_secs().unwrap_or(u64::MAX);
        secrets.retain(|s| {
            s.secret_type != secret_type || (s.expires_at >= now && s.version != stored.version)
        });

        secrets.push(stored.clone());
    }

    /// Get all secrets a token signature may legitimately be verified against.
    ///
    /// Returns the current primary plus the backup if it is still within its
    /// grace (unexpired) window, primary first. This is the verify path for
    /// finding #5: without consulting the still-valid backup, every outstanding
    /// token fails the instant a secret rotates (mass logout). The grace concept
    /// is reused verbatim from [`VersionedSecret::is_expired`] — no new notion of
    /// expiry is introduced.
    pub async fn get_verify_secrets(&self, secret_type: SecretType) -> Result<Vec<String>> {
        let mut out = Vec::with_capacity(2);

        // Primary (this also refreshes the cache on cross-process rotation).
        let primary = self.get_secret(secret_type).await?;
        out.push(primary.clone());

        // Backup, if present and still inside its grace window.
        if let Some(data) = self.storage.get(secret_type.backup_key()).await? {
            let backup: VersionedSecret = serde_json::from_slice(&data)?;
            if !backup.is_expired()? && backup.value != primary {
                out.push(backup.value);
            }
        }

        Ok(out)
    }

    /// Rotate a secret
    pub async fn rotate_secret(&self, secret_type: SecretType) -> Result<()> {
        let mut secrets = self.secrets.write().await;

        // Create new primary secret
        let new_secret = VersionedSecret::new(secret_type, &self.rotation_config)?;
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

        // Update memory cache: drop expired entries for this type, keep an
        // unexpired backup as a verify fallback, then add the new primary.
        let now = current_unix_secs()?;
        secrets.retain(|s| s.secret_type != secret_type || s.expires_at >= now);
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
            let now = current_unix_secs()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryStorage;

    fn manager() -> Arc<SecretManager> {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        Arc::new(SecretManager::new(storage))
    }

    #[tokio::test]
    async fn time_helper_and_is_expired_are_ok() {
        // Fix B: the time path must not panic and must yield Ok.
        assert!(current_unix_secs().is_ok());
        let secret = VersionedSecret::new(SecretType::JwtAuth, &SecretRotationConfig::default())
            .expect("new must not panic on the time path");
        // A freshly minted secret is within its grace window, so not expired.
        assert!(!secret.is_expired().expect("is_expired must not panic"));
    }

    #[tokio::test]
    async fn get_verify_secrets_returns_primary_only_initially() {
        let mgr = manager();
        mgr.initialize().await.unwrap();

        let secrets = mgr.get_verify_secrets(SecretType::JwtAuth).await.unwrap();
        assert_eq!(secrets.len(), 1, "no backup exists before any rotation");
        let primary = mgr.get_jwt_auth_secret().await.unwrap();
        assert_eq!(secrets[0], primary);
    }

    #[tokio::test]
    async fn get_verify_secrets_includes_unexpired_backup_after_rotation() {
        let mgr = manager();
        mgr.initialize().await.unwrap();

        let old_primary = mgr.get_jwt_auth_secret().await.unwrap();
        mgr.rotate_secret(SecretType::JwtAuth).await.unwrap();
        let new_primary = mgr.get_jwt_auth_secret().await.unwrap();
        assert_ne!(old_primary, new_primary);

        let secrets = mgr.get_verify_secrets(SecretType::JwtAuth).await.unwrap();
        assert_eq!(secrets.len(), 2, "primary + unexpired backup");
        assert_eq!(secrets[0], new_primary, "primary comes first");
        assert!(
            secrets.contains(&old_primary),
            "previous secret kept as backup during grace window"
        );
    }

    #[tokio::test]
    async fn get_verify_secrets_drops_oldest_after_double_rotation() {
        let mgr = manager();
        mgr.initialize().await.unwrap();

        let v1 = mgr.get_jwt_auth_secret().await.unwrap();
        mgr.rotate_secret(SecretType::JwtAuth).await.unwrap();
        mgr.rotate_secret(SecretType::JwtAuth).await.unwrap();
        let v3 = mgr.get_jwt_auth_secret().await.unwrap();

        let secrets = mgr.get_verify_secrets(SecretType::JwtAuth).await.unwrap();
        assert_eq!(
            secrets.len(),
            2,
            "only the latest backup generation is kept"
        );
        assert_eq!(secrets[0], v3);
        assert!(
            !secrets.contains(&v1),
            "the oldest secret is no longer accepted"
        );
    }

    #[tokio::test]
    async fn get_secret_rereads_after_external_rotation() {
        // Fix C: a manager that did not perform the rotation must still pick up
        // the new primary written to shared storage by another process.
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let mgr_a = Arc::new(SecretManager::new(Arc::clone(&storage)));
        let mgr_b = Arc::new(SecretManager::new(Arc::clone(&storage)));

        mgr_a.initialize().await.unwrap();
        // mgr_b warms its in-memory cache from the same storage.
        mgr_b.initialize().await.unwrap();
        let before = mgr_b.get_jwt_auth_secret().await.unwrap();

        // mgr_a rotates; mgr_b's cache is now stale.
        mgr_a.rotate_secret(SecretType::JwtAuth).await.unwrap();
        let rotated = mgr_a.get_jwt_auth_secret().await.unwrap();
        assert_ne!(before, rotated);

        let seen_by_b = mgr_b.get_jwt_auth_secret().await.unwrap();
        assert_eq!(
            seen_by_b, rotated,
            "stale cache must be refreshed from backing storage on version mismatch"
        );
    }
}
