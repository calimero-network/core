//! Out-of-band provisioning of the admin root key.
//!
//! The login path never mints keys (there is no trust-on-first-use and no
//! first-login bootstrap secret). Instead, the very first `user_password`
//! root key — the admin account — is created through one of these
//! operator-controlled channels, all of which end up in
//! [`provision_admin_key`]:
//!
//! * `merod init --auth-mode embedded` with admin credentials (flags, stdin,
//!   file, or the environment variables below) — the key exists before the
//!   node ever listens;
//! * process startup ([`provision_admin_from_env_if_unbootstrapped`]) from
//!   `MERO_AUTH_ADMIN_USER` / `MERO_AUTH_ADMIN_PASSWORD`, which covers
//!   in-memory storage (keys do not survive a restart), nodes initialized
//!   before credentials-at-init existed, and recovery after an auth-storage
//!   wipe;
//! * `merod auth set-admin`, the offline equivalent for a stopped node.
//!
//! The password is consumed on the spot: only the PBKDF2-derived key id and
//! the key record (username, method, permissions) are stored — nothing
//! secret is ever written to config or logs.

use std::sync::Arc;

use tracing::info;

use crate::config::{AuthConfig, UserPasswordConfig};
use crate::providers::impls::user_password::{derive_key_id, validate_password_length};
use crate::storage::models::{Key, KeyType};
use crate::storage::{KeyManager, Storage};

/// Environment variable naming the admin account to provision.
pub const ADMIN_USER_ENV: &str = "MERO_AUTH_ADMIN_USER";

/// Environment variable holding the admin password.
pub const ADMIN_PASSWORD_ENV: &str = "MERO_AUTH_ADMIN_PASSWORD";

/// Environment variable pointing at a file holding the admin password
/// (e.g. a mounted secret). Takes precedence over [`ADMIN_PASSWORD_ENV`].
pub const ADMIN_PASSWORD_FILE_ENV: &str = "MERO_AUTH_ADMIN_PASSWORD_FILE";

/// Mint the `user_password` admin root key for the given credentials.
///
/// Applies the creation-time password policy (both minimum and maximum
/// length), derives the storage key id with the same salted PBKDF2 the login
/// path uses, and stores a root key carrying the `admin` permission.
/// Idempotent for identical credentials: re-provisioning the same
/// username/password pair overwrites the same record under the same id.
///
/// Returns the key id.
pub async fn provision_admin_key(
    storage: &Arc<dyn Storage>,
    config: &UserPasswordConfig,
    username: &str,
    password: &str,
) -> eyre::Result<String> {
    if username.is_empty() {
        eyre::bail!("admin username must not be empty");
    }
    validate_password_length(
        password,
        config.min_password_length,
        config.max_password_length,
    )?;

    let key_id = derive_key_id(username, password);
    let root_key = Key::new_root_key_with_permissions(
        username.to_owned(), // the username doubles as the "public key"
        "user_password".to_owned(),
        vec!["admin".to_owned()],
        None,
    );

    let key_manager = KeyManager::new(Arc::clone(storage));
    let _was_updated = key_manager
        .set_key(&key_id, &root_key)
        .await
        .map_err(|err| eyre::eyre!("Failed to store admin root key: {err}"))?;

    info!(
        user = %crate::utils::sanitize_for_log(username),
        "Provisioned the admin root key"
    );

    Ok(key_id)
}

/// Read the admin password from the environment, if provided.
///
/// [`ADMIN_PASSWORD_FILE_ENV`] takes precedence (a single trailing newline is
/// stripped, as with any mounted secret); [`ADMIN_PASSWORD_ENV`] is the
/// fallback. Returns `Ok(None)` when neither is set (empty values count as
/// unset).
pub fn admin_password_from_env() -> eyre::Result<Option<String>> {
    if let Ok(path) = std::env::var(ADMIN_PASSWORD_FILE_ENV) {
        if !path.is_empty() {
            let raw = std::fs::read_to_string(&path).map_err(|err| {
                eyre::eyre!("failed to read {ADMIN_PASSWORD_FILE_ENV}={path}: {err}")
            })?;
            return Ok(Some(strip_trailing_newline(raw)));
        }
    }

    Ok(std::env::var(ADMIN_PASSWORD_ENV)
        .ok()
        .filter(|password| !password.is_empty()))
}

/// Read admin credentials from the environment, if provided.
///
/// Returns `Ok(None)` when [`ADMIN_USER_ENV`] is unset or empty. When it is
/// set, a password is required (see [`admin_password_from_env`]) — otherwise
/// this errors rather than silently skipping provisioning.
pub fn admin_creds_from_env() -> eyre::Result<Option<(String, String)>> {
    let username = match std::env::var(ADMIN_USER_ENV) {
        Ok(user) if !user.is_empty() => user,
        _ => return Ok(None),
    };

    match admin_password_from_env()? {
        Some(password) => Ok(Some((username, password))),
        None => eyre::bail!(
            "{ADMIN_USER_ENV} is set but no password was provided; set \
             {ADMIN_PASSWORD_ENV} or {ADMIN_PASSWORD_FILE_ENV}"
        ),
    }
}

/// Strip one trailing newline (`\n` or `\r\n`) — the artifact `echo` and most
/// secret mounts leave behind. Interior whitespace is preserved.
pub fn strip_trailing_newline(mut value: String) -> String {
    if value.ends_with('\n') {
        let _ = value.pop();
        if value.ends_with('\r') {
            let _ = value.pop();
        }
    }
    value
}

/// Provision the admin key from environment credentials when the node has no
/// root keys yet.
///
/// Called at auth-service startup (embedded and standalone). A node that
/// already has a root key never consults the environment. When no root keys
/// exist and no credentials are present, this only logs how to provision —
/// login stays disabled (fail closed) rather than minting anything.
pub async fn provision_admin_from_env_if_unbootstrapped(
    storage: &Arc<dyn Storage>,
    config: &AuthConfig,
) -> eyre::Result<()> {
    if !config
        .providers
        .get("user_password")
        .copied()
        .unwrap_or(false)
    {
        return Ok(());
    }

    let key_manager = KeyManager::new(Arc::clone(storage));
    let existing = key_manager
        .list_keys(KeyType::Root)
        .await
        .map_err(|err| eyre::eyre!("failed to determine bootstrap state: {err}"))?;
    if !existing.is_empty() {
        return Ok(());
    }

    match admin_creds_from_env()? {
        Some((username, password)) => {
            let _key_id =
                provision_admin_key(storage, &config.user_password, &username, &password).await?;
            info!("Created the admin account from {ADMIN_USER_ENV}/{ADMIN_PASSWORD_ENV}");
        }
        None => {
            info!(
                "No account exists on this node yet — login is disabled until an admin \
                 account is provisioned. Re-run `merod init` with admin credentials, run \
                 `merod auth set-admin`, or set {ADMIN_USER_ENV} and {ADMIN_PASSWORD_ENV} \
                 (or {ADMIN_PASSWORD_FILE_ENV}) and restart."
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryStorage;

    fn memory_storage() -> Arc<dyn Storage> {
        Arc::new(MemoryStorage::new())
    }

    async fn root_keys(storage: &Arc<dyn Storage>) -> Vec<(String, Key)> {
        KeyManager::new(Arc::clone(storage))
            .list_keys(KeyType::Root)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn provisions_an_admin_root_key() {
        let storage = memory_storage();
        let key_id = provision_admin_key(
            &storage,
            &UserPasswordConfig::default(),
            "admin",
            "password-1",
        )
        .await
        .unwrap();

        let keys = root_keys(&storage).await;
        assert_eq!(keys.len(), 1);
        let (stored_id, key) = &keys[0];
        assert_eq!(stored_id, &key_id);
        assert!(key.is_root_key());
        assert!(key.permissions.contains(&"admin".to_string()));
    }

    #[tokio::test]
    async fn reprovisioning_same_credentials_is_idempotent() {
        let storage = memory_storage();
        let config = UserPasswordConfig::default();
        let first = provision_admin_key(&storage, &config, "admin", "password-1")
            .await
            .unwrap();
        let second = provision_admin_key(&storage, &config, "admin", "password-1")
            .await
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(root_keys(&storage).await.len(), 1);
    }

    #[tokio::test]
    async fn creation_time_password_policy_applies() {
        // The min-length policy bites where NEW credentials are minted — here.
        let storage = memory_storage();
        let err = provision_admin_key(&storage, &UserPasswordConfig::default(), "dev", "dev")
            .await
            .expect_err("a too-short password must be rejected at provisioning time");
        assert!(
            err.to_string().contains("at least"),
            "expected a min-length error, got: {err}"
        );
        assert!(root_keys(&storage).await.is_empty());
    }

    #[tokio::test]
    async fn empty_username_is_rejected() {
        let storage = memory_storage();
        assert!(
            provision_admin_key(&storage, &UserPasswordConfig::default(), "", "password-1")
                .await
                .is_err()
        );
        assert!(root_keys(&storage).await.is_empty());
    }

    #[test]
    fn trailing_newline_is_stripped_once() {
        assert_eq!(strip_trailing_newline("secret\n".into()), "secret");
        assert_eq!(strip_trailing_newline("secret\r\n".into()), "secret");
        assert_eq!(strip_trailing_newline("secret".into()), "secret");
        assert_eq!(strip_trailing_newline("se cret\n\n".into()), "se cret\n");
    }
}
