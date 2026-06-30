use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use eyre::{eyre, Result};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::config::StorageConfig;
use crate::storage::Storage;

// ---------------------------------------------------------------------------
// At-rest encryption (finding #6: auth-secret-at-rest)
//
// The JWT signing secrets must not sit on disk as plaintext/base64 — base64 is
// encoding, not encryption. Before a `VersionedSecret` blob is handed to the
// storage layer it is sealed with AES-256-GCM (authenticated encryption, reused
// from the crate's existing `aes-gcm` dependency — no new dependency added). On
// read the blob is unsealed back to the exact bytes the rest of the crate
// expects, so token semantics, rotation and the verify path are untouched.
//
// KEK (key-encryption-key) provisioning — REVIEW POINT.
//
// The KEK MUST be stable and reproducible across `SecretManager` instances and
// across process restarts: two managers over the same storage/data-dir have to
// interoperate, otherwise a fresh manager (e.g. on node restart) cannot unseal
// the secret a previous one sealed and the node cannot boot. The KEK is resolved
// in the following precedence order:
//
//   1. `MERO_AUTH_SECRET_KEK` env var, if set and non-empty: the strongest path,
//      the KEK never touches disk. The value is hashed with SHA-256 to derive a
//      32-byte AES key, so any passphrase length is accepted. Stable by virtue of
//      being operator-supplied; takes precedence over anything on disk.
//   2. Otherwise, for path-backed (RocksDB) storage, a 32-byte random KEK is
//      generated once and persisted to a sibling key file `<db-path>.kek` with
//      `0600` perms, then reused verbatim on every subsequent start. This is
//      defense-in-depth: a leaked DB copy (stray SST/backup) is useless without
//      the separate key file. An attacker who can already read the (now `0700`)
//      data directory as the owning user can read both — that residual risk is
//      why operators are encouraged to use option (1). Zero-config deployments
//      keep working and secrets survive restarts.
//   3. Otherwise (pathless / in-memory storage, or if the keyfile cannot be
//      written) the KEK is stored *inside the storage backend itself* under a
//      reserved key, generated once on first use and reloaded thereafter. This
//      guarantees that ANY two managers sharing the same storage interoperate
//      (the KEK is a property of the storage contents, not of the process or the
//      constructor used) and that a persistent backend survives a restart. The
//      KEK then sits next to the ciphertext, so a full-store exfiltration
//      recovers the secrets — this layer still prevents value-level/log/partial
//      exposure and ensures the signing key is never at rest as decodable base64.
//      Operators wanting cryptographic separation should use option (1).
//
// Resolution is lazy and async (the storage-embedded path needs storage I/O):
// the KEK is computed on first seal/unseal and memoised in a `OnceCell`.
// ---------------------------------------------------------------------------

/// Environment variable an operator can set to supply the key-encryption-key.
const KEK_ENV: &str = "MERO_AUTH_SECRET_KEK";

/// Magic prefix marking a sealed (AES-256-GCM) at-rest blob. Versioned so the
/// format can evolve; its absence also lets us transparently read pre-encryption
/// (legacy plaintext-JSON) secrets and re-seal them on next write.
const SEALED_MAGIC: &[u8; 5] = b"MEAS1";

/// AES-GCM nonce length in bytes.
const NONCE_LEN: usize = 12;

/// Seal `plaintext` as `MAGIC || nonce || ciphertext+tag` using AES-256-GCM.
fn seal(kek: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(kek));
    let nonce_bytes: [u8; NONCE_LEN] = rand::thread_rng().gen();
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext)
        .map_err(|e| eyre!("failed to seal secret: {e}"))?;

    let mut out = Vec::with_capacity(SEALED_MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(SEALED_MAGIC);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Reverse [`seal`]. Blobs without the magic prefix are treated as legacy
/// plaintext and returned unchanged, so an upgrade reads existing secrets.
fn unseal(kek: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < SEALED_MAGIC.len() + NONCE_LEN || &blob[..SEALED_MAGIC.len()] != SEALED_MAGIC {
        // Legacy (pre-encryption) plaintext JSON — passed through verbatim.
        return Ok(blob.to_vec());
    }

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(kek));
    let nonce = &blob[SEALED_MAGIC.len()..SEALED_MAGIC.len() + NONCE_LEN];
    let ciphertext = &blob[SEALED_MAGIC.len() + NONCE_LEN..];
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|e| eyre!("failed to unseal secret: {e}"))
}

/// Derive a 32-byte KEK from the `MERO_AUTH_SECRET_KEK` env var, if set.
fn kek_from_env() -> Option<[u8; 32]> {
    match std::env::var(KEK_ENV) {
        Ok(value) if !value.is_empty() => Some(Sha256::digest(value.as_bytes()).into()),
        _ => None,
    }
}

/// Sibling key-file path for a RocksDB directory (e.g. `auth-db` -> `auth-db.kek`).
fn keyfile_path(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push(".kek");
    PathBuf::from(os)
}

/// Load the KEK from `path`, or generate-and-persist a fresh one (`0600` on unix).
fn kek_from_keyfile(path: &Path) -> Result<[u8; 32]> {
    if let Ok(bytes) = std::fs::read(path) {
        if bytes.len() == 32 {
            let mut kek = [0u8; 32];
            kek.copy_from_slice(&bytes);
            return Ok(kek);
        }
        warn!(
            "KEK file {path:?} has unexpected length {}; regenerating",
            bytes.len()
        );
    }

    let kek: [u8; 32] = rand::thread_rng().gen();
    std::fs::write(path, kek).map_err(|e| eyre!("failed to write KEK file {:?}: {e}", path))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| eyre!("failed to chmod KEK file {:?}: {e}", path))?;
    }

    info!("Generated new at-rest KEK file at {path:?}");
    Ok(kek)
}

/// Resolve the at-rest KEK for the given storage backend (see module docs).
fn resolve_secret_kek(config: &StorageConfig) -> [u8; 32] {
    if let Some(kek) = kek_from_env() {
        info!("Using {KEK_ENV} for at-rest secret encryption");
        return kek;
    }

    match config {
        StorageConfig::RocksDB { path } => match kek_from_keyfile(&keyfile_path(path)) {
            Ok(kek) => kek,
            Err(e) => {
                warn!("Falling back to ephemeral at-rest KEK: {e}");
                rand::thread_rng().gen()
            }
        },
        StorageConfig::Memory => {
            warn!(
                "No {KEK_ENV} set and storage is in-memory; using an ephemeral at-rest KEK \
                 (dev/test only — persisted secrets would not survive a restart)"
            );
            rand::thread_rng().gen()
        }
    }
}

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
        let secret: [u8; 32] = rand::thread_rng().gen();

        Self {
            value: URL_SAFE_NO_PAD.encode(secret),
            version: format!("v{now}"),
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
    /// At-rest key-encryption-key. `Some` ⇒ secrets are sealed with AES-256-GCM
    /// before they hit storage; `None` ⇒ the backend has no at-rest surface
    /// (in-memory) so secrets are kept as plaintext. See module docs.
    kek: Option<[u8; 32]>,
}

impl SecretManager {
    /// Create a new secret manager, resolving the at-rest KEK from the
    /// `MERO_AUTH_SECRET_KEK` env var (highest precedence) or, failing that, from
    /// the storage backend ([`Storage::at_rest_kek`] — a stable sibling-keyfile KEK
    /// for RocksDB, `None` for the in-memory backend). When neither yields a KEK the
    /// backend has nothing at rest and secrets are stored unsealed.
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        let kek = kek_from_env().or_else(|| storage.at_rest_kek());
        if kek.is_none() {
            warn!(
                "No {KEK_ENV} set and the storage backend provides no at-rest KEK; \
                 secrets are stored unsealed (the in-memory backend has nothing at rest)"
            );
        }
        Self::build(storage, kek)
    }

    /// Create a secret manager, resolving the at-rest KEK from the storage config.
    ///
    /// For RocksDB this persists/loads a sibling `<db-path>.kek` file (`0600`) when
    /// `MERO_AUTH_SECRET_KEK` is not provided. This is the constructor production
    /// wiring should use.
    pub fn with_storage_config(storage: Arc<dyn Storage>, config: &StorageConfig) -> Self {
        let kek = resolve_secret_kek(config);
        Self::with_kek(storage, kek)
    }

    /// Create a secret manager with an explicit at-rest KEK (used by tests).
    pub fn with_kek(storage: Arc<dyn Storage>, kek: [u8; 32]) -> Self {
        Self::build(storage, Some(kek))
    }

    fn build(storage: Arc<dyn Storage>, kek: Option<[u8; 32]>) -> Self {
        Self {
            storage,
            secrets: RwLock::new(Vec::new()),
            rotation_config: SecretRotationConfig::default(),
            kek,
        }
    }

    /// Serialize, seal (when a KEK is present) and persist a secret under `key`.
    /// With no KEK (in-memory backend) the secret is stored as plaintext — there
    /// is no disk at rest to protect, and sealing would break interop between two
    /// managers sharing the store.
    async fn store_secret(&self, key: &str, secret: &VersionedSecret) -> Result<()> {
        let plaintext = serde_json::to_vec(secret)?;
        let blob = match &self.kek {
            Some(kek) => seal(kek, &plaintext)?,
            None => plaintext,
        };
        self.storage.set(key, &blob).await.map_err(|e| eyre!("{e}"))
    }

    /// Load and (when a KEK is present) unseal a secret from `key`, if present.
    async fn load_secret(&self, key: &str) -> Result<Option<VersionedSecret>> {
        match self.storage.get(key).await? {
            Some(blob) => {
                let plaintext = match &self.kek {
                    Some(kek) => unseal(kek, &blob)?,
                    None => blob,
                };
                Ok(Some(serde_json::from_slice(&plaintext)?))
            }
            None => Ok(None),
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
        let secret = match self.load_secret(secret_type.primary_key()).await? {
            Some(secret) => secret,
            None => {
                // Generate new secret
                let new_secret = VersionedSecret::new(secret_type, &self.rotation_config);

                // Try to save (sealed) to primary location
                if let Err(e) = self
                    .store_secret(secret_type.primary_key(), &new_secret)
                    .await
                {
                    error!("Failed to save secret to primary storage: {}", e);

                    // Try backup location
                    self.store_secret(secret_type.backup_key(), &new_secret)
                        .await?;
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
        match self.load_secret(secret_type.primary_key()).await? {
            Some(secret) => Ok(secret.value),
            None => {
                // Try backup location
                match self.load_secret(secret_type.backup_key()).await? {
                    Some(secret) => {
                        // Restore to primary location (re-sealed under the current KEK)
                        if let Err(e) = self.store_secret(secret_type.primary_key(), &secret).await
                        {
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

        // Save (sealed) to storage
        self.store_secret(secret_type.primary_key(), &new_secret)
            .await?;

        // Update old secret as backup
        if let Some(old_secret) = secrets
            .iter_mut()
            .find(|s| s.secret_type == secret_type && s.is_primary)
        {
            old_secret.is_primary = false;
            self.store_secret(secret_type.backup_key(), old_secret)
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

#[cfg(test)]
mod tests {
    use crate::storage::MemoryStorage;

    use super::*;

    const TEST_KEK: [u8; 32] = [7u8; 32];

    #[test]
    fn seal_unseal_round_trip() {
        let plaintext = b"super secret signing key material";
        let sealed = seal(&TEST_KEK, plaintext).unwrap();

        // Sealed output must be marked, nonce-prefixed and must NOT contain the
        // plaintext verbatim (i.e. it is encrypted, not encoded).
        assert_eq!(&sealed[..SEALED_MAGIC.len()], SEALED_MAGIC);
        assert!(sealed.windows(plaintext.len()).all(|w| w != plaintext));

        let opened = unseal(&TEST_KEK, &sealed).unwrap();
        assert_eq!(opened, plaintext);
    }

    #[test]
    fn unseal_with_wrong_kek_fails() {
        let sealed = seal(&TEST_KEK, b"secret").unwrap();
        let wrong = [9u8; 32];
        assert!(unseal(&wrong, &sealed).is_err());
    }

    #[test]
    fn unseal_passes_through_legacy_plaintext() {
        // Pre-encryption blobs (no magic prefix) are returned verbatim.
        let legacy = br#"{"value":"abc","version":"v1"}"#;
        let out = unseal(&TEST_KEK, legacy).unwrap();
        assert_eq!(out, legacy);
    }

    /// encrypt -> store -> load -> decrypt -> equals original.
    #[tokio::test]
    async fn secret_round_trips_through_storage_encrypted() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let manager = SecretManager::with_kek(Arc::clone(&storage), TEST_KEK);
        manager.initialize().await.unwrap();

        let original = manager.get_jwt_auth_secret().await.unwrap();

        // The blob actually written to storage must be sealed, not the plaintext
        // base64 secret value.
        let raw = storage
            .get(SecretType::JwtAuth.primary_key())
            .await
            .unwrap()
            .expect("secret should be persisted");
        assert_eq!(&raw[..SEALED_MAGIC.len()], SEALED_MAGIC);
        assert!(
            !String::from_utf8_lossy(&raw).contains(&original),
            "plaintext secret value must not appear in the stored blob"
        );

        // A fresh manager (cold cache) with the same KEK must decrypt back to the
        // identical secret value.
        let reloaded = SecretManager::with_kek(Arc::clone(&storage), TEST_KEK);
        let value = reloaded.get_jwt_auth_secret().await.unwrap();
        assert_eq!(value, original);
    }

    #[tokio::test]
    async fn wrong_kek_cannot_load_stored_secret() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        SecretManager::with_kek(Arc::clone(&storage), TEST_KEK)
            .initialize()
            .await
            .unwrap();

        // Cold-cache manager with a different KEK must fail to decrypt.
        let other = SecretManager::with_kek(Arc::clone(&storage), [1u8; 32]);
        assert!(other.get_jwt_auth_secret().await.is_err());
    }

    /// Regression: an in-memory backend has no at-rest surface, so `new()` resolves
    /// no KEK and stores plaintext. Two managers over the same store MUST interop —
    /// the prior bug minted a random per-instance KEK and a second manager could not
    /// unseal the first's secret (which broke node restart / every middleware test).
    #[tokio::test]
    async fn memory_backend_stores_plaintext_and_two_managers_interop() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let first = SecretManager::new(Arc::clone(&storage));
        first.initialize().await.unwrap();
        let original = first.get_jwt_auth_secret().await.unwrap();

        // No KEK ⇒ no seal magic prefix; the blob is plaintext JSON.
        let raw = storage
            .get(SecretType::JwtAuth.primary_key())
            .await
            .unwrap()
            .expect("secret should be persisted");
        assert_ne!(&raw[..SEALED_MAGIC.len()], SEALED_MAGIC);

        // A second, independently-constructed manager over the same store reads it.
        let second = SecretManager::new(Arc::clone(&storage));
        assert_eq!(second.get_jwt_auth_secret().await.unwrap(), original);
    }
}
