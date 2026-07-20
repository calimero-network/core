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

use tracing::{info, warn};

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
/// including pre-PBKDF2 legacy ids) is deleted — along with every scoped
/// client key derived from it — so re-provisioning rotates the password and
/// invalidates tokens minted under the old one rather than leaving either
/// valid forever. Keys for other usernames are untouched. Idempotent for
/// identical credentials.
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
        let same_provider = match key.auth_method.as_deref() {
            Some(method) => USER_PASSWORD_METHODS.contains(&method),
            // Legacy keys predate the `auth_method` field being populated (the
            // "pre-PBKDF2 legacy ids" this function's doc promises to rotate
            // out). A root key whose public key is this username but has no
            // recorded auth_method is such a key: `user_password` is the only
            // provider that mints username-keyed root keys, so treat it as
            // belonging here rather than leaving an old credential able to
            // authenticate forever. `same_user` already pins it to this exact
            // username, so this can't sweep some other identity.
            None => true,
        };
        if stale_id != key_id && same_user && same_provider {
            // Revoke the client keys derived from the superseded root key
            // first: they authenticate independently of their parent, so a
            // token minted under the old password would otherwise survive the
            // rotation. Do this before deleting the root key — deleting the
            // root drops the root→client index this lookup relies on.
            let revoked_clients = key_manager
                .delete_client_keys_for_root(&stale_id)
                .await
                .map_err(|err| {
                    eyre::eyre!("Failed to revoke client keys of the superseded root key: {err}")
                })?;
            key_manager.delete_key(&stale_id).await.map_err(|err| {
                eyre::eyre!("Failed to delete the superseded root key for this username: {err}")
            })?;
            info!(
                user = %crate::utils::sanitize_for_log(username),
                revoked_clients,
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
/// fallback. Returns `Ok(None)` when neither yields a password. An empty value
/// counts as unset for both sources: a blank or newline-only secret file is
/// treated as "not provided" and **falls through** to [`ADMIN_PASSWORD_ENV`]
/// (a secret mount that hasn't been populated yet must not shadow a valid
/// fallback), so it can never mint a password-less admin even where the
/// min-length policy is relaxed to 0. An unreadable file is still a hard error
/// — that is a misconfiguration, not an "unset" source.
pub fn admin_password_from_env() -> eyre::Result<Option<String>> {
    if let Ok(path) = std::env::var(ADMIN_PASSWORD_FILE_ENV) {
        if !path.is_empty() {
            let raw = std::fs::read_to_string(&path).map_err(|err| {
                eyre::eyre!("failed to read {ADMIN_PASSWORD_FILE_ENV}={path}: {err}")
            })?;
            let password = strip_trailing_newline(raw);
            if !password.is_empty() {
                return Ok(Some(password));
            }
            // Blank/newline-only file: fall through to the plain env var.
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

/// Whether any admin credential environment variable is set to a non-empty
/// value. Used only to decide whether to warn that credentials are being
/// ignored (e.g. the `user_password` provider is disabled); it deliberately
/// does not read secret files or validate a full pair.
fn admin_env_present() -> bool {
    let non_empty = |name: &str| std::env::var(name).is_ok_and(|value| !value.is_empty());
    non_empty(ADMIN_USER_ENV) || non_empty(ADMIN_PASSWORD_ENV) || non_empty(ADMIN_PASSWORD_FILE_ENV)
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
        // Fail-loud: the whole point of this path is that credentials are
        // never silently dropped. If an operator wired admin env creds through
        // but the provider that would consume them is off, say so — otherwise
        // they debug a locked-out node with no clue why the env had no effect.
        if admin_env_present() {
            warn!(
                "{ADMIN_USER_ENV}/{ADMIN_PASSWORD_ENV} set but the `user_password` provider is \
                 disabled; the admin account cannot be provisioned and these credentials are \
                 ignored. Enable the provider to use them."
            );
        }
        return Ok(());
    }

    // Does a user_password admin already exist? This MUST use the same
    // definition of "belongs to user_password" as `provision_admin_key`'s
    // rotation sweep, or the two disagree: rotation treats a root key with no
    // auth_method as a legacy user_password key, so bootstrap detection must
    // too. Otherwise a node whose only admin is a pre-PBKDF2 legacy key (no
    // auth_method) reads as un-bootstrapped here, env-provisioning runs, and
    // the rotation logic silently deletes that legacy admin — a routine
    // restart with the env vars still set (common in containers) would
    // replace an existing account. A root key minted by some FUTURE other
    // provider still doesn't count: providers tag their own auth_method, so it
    // is neither a user_password method nor untagged.
    let key_manager = KeyManager::new(Arc::clone(storage));
    let has_admin = key_manager
        .has_any_key(KeyType::Root, Some(&USER_PASSWORD_METHODS))
        .await
        .map_err(|err| eyre::eyre!("failed to determine provisioning state: {err}"))?
        || key_manager
            .has_any_key(KeyType::Root, Some(&[]))
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
    use std::sync::Mutex;

    use super::*;
    use crate::storage::MemoryStorage;

    /// Serializes tests that mutate the process-global admin env vars. The test
    /// harness runs tests in parallel threads and env vars are process-wide, so
    /// any two tests touching `MERO_AUTH_ADMIN_*` must not overlap. Poisoning is
    /// recovered (a panic in one env test must not wedge the others).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

        // The username→key_id index must still resolve to the CURRENT key:
        // deleting the old key (whose public_key is the same username) must not
        // clobber the index entry the new key just claimed.
        let resolved = KeyManager::new(Arc::clone(&storage))
            .find_root_key_by_public_key("admin")
            .await
            .unwrap();
        assert_eq!(
            resolved.map(|(id, _)| id),
            Some(new_id),
            "the public-key index must point at the rotated-in key"
        );
    }

    #[tokio::test]
    async fn rotation_sweeps_a_legacy_key_with_no_auth_method() {
        // Pre-PBKDF2 keys predate the auth_method field: a root key for this
        // username with auth_method == None must still be rotated out, or an
        // old credential keeps authenticating forever (the doc promises it is
        // swept).
        let storage = memory_storage();
        let config = UserPasswordConfig::default();
        let key_manager = KeyManager::new(Arc::clone(&storage));

        let mut legacy = Key::new_root_key_with_permissions(
            "admin".to_owned(),
            "user_password".to_owned(),
            vec!["admin".to_owned()],
            None,
        );
        legacy.auth_method = None; // as an old on-disk record would be
        key_manager.set_key("legacy-id", &legacy).await.unwrap();

        let new_id = provision_admin_key(&storage, &config, "admin", "new-password-2")
            .await
            .unwrap();

        let keys = root_keys(&storage).await;
        assert_eq!(keys.len(), 1, "the legacy key must be swept");
        assert_eq!(keys[0].0, new_id);
    }

    // Holds the std Mutex across `.await`: a test-only serialization guard for
    // the process-global env vars, on a per-test current-thread runtime with no
    // other task contending it — none of the deadlock hazard the lint guards.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_legacy_only_node_reads_as_already_bootstrapped() {
        // Bootstrap detection must count a legacy key (auth_method == None) as
        // an existing admin, or env-provisioning runs on restart and the
        // rotation sweep silently replaces the legacy account.
        let _env = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let storage = memory_storage();
        let key_manager = KeyManager::new(Arc::clone(&storage));

        let mut legacy = Key::new_root_key_with_permissions(
            "admin".to_owned(),
            "user_password".to_owned(),
            vec!["admin".to_owned()],
            None,
        );
        legacy.auth_method = None;
        key_manager.set_key("legacy-id", &legacy).await.unwrap();

        // Env vars a container would keep passing on every restart.
        std::env::set_var(ADMIN_USER_ENV, "admin");
        std::env::set_var(ADMIN_PASSWORD_ENV, "env-password-123");
        let result = provision_admin_from_env_if_unbootstrapped(
            &storage,
            &crate::embedded::default_config(),
        )
        .await;
        std::env::remove_var(ADMIN_USER_ENV);
        std::env::remove_var(ADMIN_PASSWORD_ENV);
        result.unwrap();

        let keys = root_keys(&storage).await;
        assert_eq!(keys.len(), 1, "the legacy admin must be left untouched");
        assert_eq!(
            keys[0].0, "legacy-id",
            "env-provisioning must not run against a legacy-only node"
        );
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

    #[tokio::test]
    async fn rotating_the_password_revokes_client_keys_of_the_old_root() {
        // A client token minted under the compromised password must stop
        // working once set-admin rotates the password — otherwise rotation as
        // an incident-response tool is defeated.
        let storage = memory_storage();
        let config = UserPasswordConfig::default();
        let key_manager = KeyManager::new(Arc::clone(&storage));

        let old_id = provision_admin_key(&storage, &config, "admin", "old-password-1")
            .await
            .unwrap();

        // A scoped client key derived from the old admin root key.
        let client = Key::new_client_key(old_id.clone(), "cli".to_owned(), vec![], None);
        key_manager.set_key("client-token", &client).await.unwrap();
        assert!(key_manager.get_key("client-token").await.unwrap().is_some());

        // Rotate the admin password.
        let new_id = provision_admin_key(&storage, &config, "admin", "new-password-2")
            .await
            .unwrap();
        assert_ne!(old_id, new_id);

        // Old root gone, its client gone, only the new root remains.
        assert!(key_manager.get_key("client-token").await.unwrap().is_none());
        let keys = root_keys(&storage).await;
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].0, new_id);
    }

    #[test]
    fn admin_password_from_env_file_precedence_and_fallthrough() {
        // This is the only test that touches the admin password env vars, so it
        // owns them for its duration and covers every branch sequentially.
        let _env = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir();
        let blank = dir.join("mero-auth-blank-admin-password-test");
        let filled = dir.join("mero-auth-filled-admin-password-test");
        std::fs::write(&blank, "\n").unwrap();
        std::fs::write(&filled, "from-file\n").unwrap();

        std::env::remove_var(ADMIN_PASSWORD_ENV);
        std::env::remove_var(ADMIN_PASSWORD_FILE_ENV);

        // 1. A blank file with no env fallback resolves to None — never a
        //    password-less admin.
        std::env::set_var(ADMIN_PASSWORD_FILE_ENV, &blank);
        assert_eq!(admin_password_from_env().unwrap(), None);

        // 2. A blank file falls through to the plain env var (an unpopulated
        //    secret mount must not shadow a valid fallback).
        std::env::set_var(ADMIN_PASSWORD_ENV, "from-env");
        assert_eq!(
            admin_password_from_env().unwrap().as_deref(),
            Some("from-env")
        );

        // 3. A non-empty file wins over the env var.
        std::env::set_var(ADMIN_PASSWORD_FILE_ENV, &filled);
        assert_eq!(
            admin_password_from_env().unwrap().as_deref(),
            Some("from-file")
        );

        std::env::remove_var(ADMIN_PASSWORD_ENV);
        std::env::remove_var(ADMIN_PASSWORD_FILE_ENV);
        let _ = std::fs::remove_file(&blank);
        let _ = std::fs::remove_file(&filled);
    }

    #[test]
    fn trailing_newline_is_stripped_once() {
        assert_eq!(strip_trailing_newline("secret\n".into()), "secret");
        assert_eq!(strip_trailing_newline("secret\r\n".into()), "secret");
        assert_eq!(strip_trailing_newline("secret".into()), "secret");
        assert_eq!(strip_trailing_newline("se cret\n\n".into()), "se cret\n");
    }
}
