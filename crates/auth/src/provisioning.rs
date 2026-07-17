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

/// Auth-method names the `user_password` provider answers to; used to scope
/// key lookups and rotation to this provider's keys only.
const USER_PASSWORD_METHODS: [&str; 2] = ["user_password", "username_password"];

/// Mint the `user_password` admin root key for the given credentials.
///
/// Applies the creation-time password policy (both minimum and maximum
/// length), derives the storage key id with the same salted PBKDF2 the login
/// path uses, and stores a root key carrying the `admin` permission.
///
/// This SETS the account for `username`: any other `user_password` root key
/// stored for the same username (i.e. minted under a previous password,
/// including pre-PBKDF2 legacy ids) is deleted, so re-provisioning rotates
/// the password rather than leaving the old one valid forever. Keys for
/// other usernames are untouched. Idempotent for identical credentials.
///
/// The new key is stored before the old ones are deleted, so a crash in
/// between leaves both passwords valid (re-run to finish the rotation) —
/// never an account-less node.
///
/// Returns the key id.
pub async fn provision_admin_key(
    storage: &Arc<dyn Storage>,
    config: &UserPasswordConfig,
    username: &str,
    password: &str,
) -> eyre::Result<String> {
    validate_admin_credentials(config, username, password)?;

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

    // Rotate: drop every other user_password key stored for this username.
    // Without this, provisioning a new password after a compromise would
    // leave the compromised one authenticating forever (its derived key id
    // is still a valid lookup).
    let existing = key_manager
        .list_keys(KeyType::Root)
        .await
        .map_err(|err| eyre::eyre!("Failed to list root keys for rotation: {err}"))?;
    for (stale_id, key) in existing {
        let same_user = key.public_key.as_deref() == Some(username);
        let same_provider = key
            .auth_method
            .as_deref()
            .is_some_and(|method| USER_PASSWORD_METHODS.contains(&method));
        if stale_id != key_id && same_user && same_provider {
            key_manager.delete_key(&stale_id).await.map_err(|err| {
                eyre::eyre!("Failed to delete the superseded root key for this username: {err}")
            })?;
            info!(
                user = %crate::utils::sanitize_for_log(username),
                "Rotated out a superseded root key for this username"
            );
        }
    }

    info!(
        user = %crate::utils::sanitize_for_log(username),
        "Provisioned the admin root key"
    );

    Ok(key_id)
}

/// Validate admin credentials against the creation-time policy (non-empty
/// username; password within the configured min/max length) without touching
/// storage.
///
/// [`provision_admin_key`] applies it itself; callers that do destructive
/// work before minting (e.g. `merod init --force`, which wipes the node home)
/// call it FIRST so bad credentials fail before anything is destroyed.
pub fn validate_admin_credentials(
    config: &UserPasswordConfig,
    username: &str,
    password: &str,
) -> eyre::Result<()> {
    if username.is_empty() {
        eyre::bail!("admin username must not be empty");
    }
    validate_password_length(
        password,
        config.min_password_length,
        config.max_password_length,
    )
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
/// Returns `Ok(None)` only when NEITHER side is set. A partial
/// specification errors instead of being silently ignored — an operator who
/// wired only one of the two variables through (easy in container/CI
/// configs) must find out at startup, not from a node that quietly has no
/// admin account.
pub fn admin_creds_from_env() -> eyre::Result<Option<(String, String)>> {
    let username = std::env::var(ADMIN_USER_ENV)
        .ok()
        .filter(|user| !user.is_empty());
    let password = admin_password_from_env()?;

    match (username, password) {
        (Some(username), Some(password)) => Ok(Some((username, password))),
        (Some(_), None) => eyre::bail!(
            "{ADMIN_USER_ENV} is set but no password was provided; set \
             {ADMIN_PASSWORD_ENV} or {ADMIN_PASSWORD_FILE_ENV}"
        ),
        (None, Some(_)) => eyre::bail!(
            "an admin password is set ({ADMIN_PASSWORD_ENV} or \
             {ADMIN_PASSWORD_FILE_ENV}) but {ADMIN_USER_ENV} is not; set it \
             or unset the password"
        ),
        (None, None) => Ok(None),
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

    // Only user_password keys count: a root key minted by some future other
    // provider must not silently suppress provisioning the user_password
    // admin this function exists to create.
    let key_manager = KeyManager::new(Arc::clone(storage));
    let has_admin = key_manager
        .has_any_key(KeyType::Root, Some(&USER_PASSWORD_METHODS))
        .await
        .map_err(|err| eyre::eyre!("failed to determine provisioning state: {err}"))?;
    if has_admin {
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
    async fn reprovisioning_same_username_rotates_the_password() {
        // set-admin with a new password must revoke the old one: after
        // rotation only the new key remains, so the compromised/old password
        // can no longer authenticate.
        let storage = memory_storage();
        let config = UserPasswordConfig::default();
        let old_id = provision_admin_key(&storage, &config, "admin", "old-password-1")
            .await
            .unwrap();
        let new_id = provision_admin_key(&storage, &config, "admin", "new-password-2")
            .await
            .unwrap();
        assert_ne!(old_id, new_id);

        let keys = root_keys(&storage).await;
        assert_eq!(keys.len(), 1, "the superseded key must be deleted");
        assert_eq!(keys[0].0, new_id);
    }

    #[tokio::test]
    async fn different_usernames_keep_separate_admin_keys() {
        // Rotation is per-username: provisioning a second admin under a
        // different name must not touch the first.
        let storage = memory_storage();
        let config = UserPasswordConfig::default();
        let _ops = provision_admin_key(&storage, &config, "ops", "password-1")
            .await
            .unwrap();
        let _rescue = provision_admin_key(&storage, &config, "rescue", "password-2")
            .await
            .unwrap();
        assert_eq!(root_keys(&storage).await.len(), 2);
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
