use std::any::Any;
use std::num::NonZeroU32;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use ring::pbkdf2;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{debug, error, info, warn};
use validator::Validate;

use crate::api::handlers::auth::TokenRequest;
use crate::auth::token::TokenManager;
use crate::config::{AuthConfig, UserPasswordConfig};
use crate::providers::core::provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};
use crate::providers::core::provider_data_registry::AuthDataType;
use crate::providers::core::provider_registry::ProviderRegistration;
use crate::providers::ProviderContext;
use crate::storage::models::Key;
use crate::storage::{KeyManager, Storage};
use crate::{register_auth_data_type, register_auth_provider, AuthResponse};

/// Application-wide salt prefix for deriving the username/password key id.
///
/// The key id doubles as the storage lookup key, so it must be reproducible
/// from the credentials alone, with no per-user state stored before lookup.
/// A per-user *random* salt is therefore impossible in this model. We instead
/// derive a per-user salt deterministically as `KEY_ID_SALT_PREFIX || username`
/// (the username is known at lookup time), which defeats cross-user precomputed
/// (rainbow) tables, and rely on the PBKDF2 iteration count for offline
/// brute-force resistance. Tradeoff: because the salt is derived (not random),
/// an attacker who learns the scheme can still mount a per-target attack, but
/// that attack is now PBKDF2-stretched rather than a single unsalted SHA256.
const KEY_ID_SALT_PREFIX: &[u8] = b"calimero:auth:user_password:key-id:v1:";

/// PBKDF2 iteration count for key-id derivation.
const KEY_ID_PBKDF2_ITERATIONS: u32 = 100_000;

/// Length of the derived key-id, in bytes (256-bit, hex-encoded to 64 chars).
const KEY_ID_LEN: usize = 32;

/// Deterministically derive the storage key id from credentials using a
/// per-user-salted PBKDF2-HMAC-SHA256, replacing the previous unsalted SHA256.
///
/// `pub(crate)` so the offline provisioning path ([`crate::provisioning`]) can
/// mint the admin root key under exactly the id the login path will look up.
pub(crate) fn derive_key_id(username: &str, password: &str) -> String {
    // Per-user deterministic salt: fixed domain-separation prefix + username.
    let mut salt = Vec::with_capacity(KEY_ID_SALT_PREFIX.len() + username.len());
    salt.extend_from_slice(KEY_ID_SALT_PREFIX);
    salt.extend_from_slice(username.as_bytes());

    // Iteration count is a non-zero compile-time constant.
    let iterations = NonZeroU32::new(KEY_ID_PBKDF2_ITERATIONS)
        .expect("KEY_ID_PBKDF2_ITERATIONS must be non-zero");

    let mut out = [0u8; KEY_ID_LEN];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        &salt,
        password.as_bytes(),
        &mut out,
    );
    hex::encode(out)
}

/// The pre-PBKDF2 key-id derivation: an unsalted `SHA256("user_password:{u}:{p}")`.
///
/// Retained ONLY so that a node upgraded from a release that used this scheme
/// can still find the root key it already stored, and transparently re-key it to
/// the salted derivation on the next successful login (see
/// [`UserPasswordProvider::verify_credentials`]). Never used to *create* a key.
fn legacy_key_id(username: &str, password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("user_password:{username}:{password}").as_bytes());
    hex::encode(hasher.finalize())
}

/// Enforce configured password length bounds.
///
/// Returns a clear validation error when the password is shorter than
/// `min_length` or longer than `max_length`. Length is measured in Unicode
/// scalar values (`chars`), not bytes.
///
/// `pub(crate)` so [`crate::provisioning`] applies the same creation-time
/// policy when minting the admin key out of band.
pub(crate) fn validate_password_length(
    password: &str,
    min_length: usize,
    max_length: usize,
) -> eyre::Result<()> {
    let len = password.chars().count();
    if len < min_length {
        eyre::bail!("Password must be at least {min_length} characters long");
    }
    if len > max_length {
        eyre::bail!("Password must be at most {max_length} characters long");
    }
    Ok(())
}

/// Guard the KDF against absurdly long inputs on the *authentication* path.
///
/// The minimum length is a policy for NEW credentials and is deliberately NOT
/// enforced here: an existing user whose password predates the policy must still
/// be able to log in (enforcing the minimum at login would lock them out of
/// their own node, with no recovery path). The maximum is still enforced because
/// it bounds PBKDF2 work per request.
fn validate_password_for_auth(password: &str, max_length: usize) -> eyre::Result<()> {
    validate_password_length(password, 0, max_length)
}

/// Username/password authentication data
///
/// Older clients may still send a `bootstrap_secret` field (the removed
/// first-login setup-code flow); serde ignores unknown fields, so those
/// payloads keep parsing and the value is simply discarded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPasswordAuthData {
    /// Username
    pub username: String,
    /// Password (will be hashed)
    pub password: String,
}

/// Username/password auth data type for the registry
pub struct UserPasswordAuthDataType;

impl AuthDataType for UserPasswordAuthDataType {
    fn method_name(&self) -> &str {
        "user_password"
    }

    fn parse_from_value(&self, value: Value) -> eyre::Result<Box<dyn std::any::Any + Send + Sync>> {
        // Try to deserialize as UserPasswordAuthData
        match serde_json::from_value::<UserPasswordAuthData>(value) {
            Ok(data) => Ok(Box::new(data)),
            Err(err) => Err(eyre::eyre!("Invalid username/password auth data: {}", err)),
        }
    }

    fn get_sample_structure(&self) -> Value {
        serde_json::json!({
            "username": "example_user",
            "password": "example_password"
        })
    }
}

/// Username/password authentication provider
pub struct UserPasswordProvider {
    storage: Arc<dyn Storage>,
    key_manager: KeyManager,
    token_manager: TokenManager,
    config: UserPasswordConfig,
}

impl UserPasswordProvider {
    /// Create a new username/password provider
    pub fn new(context: ProviderContext, config: UserPasswordConfig) -> Self {
        Self {
            storage: context.storage,
            key_manager: context.key_manager,
            token_manager: context.token_manager,
            config,
        }
    }

    /// Hash username and password to create a unique key ID
    ///
    /// # Arguments
    ///
    /// * `username` - The username
    /// * `password` - The password
    ///
    /// # Returns
    ///
    /// * `String` - The generated key ID
    fn generate_key_id(&self, username: &str, password: &str) -> String {
        derive_key_id(username, password)
    }

    /// Enforce the configured password length bounds for this provider.
    ///
    /// Creation-path policy (trait-level `create_root_key`) — enforces BOTH
    /// the minimum and the maximum.
    fn validate_password(&self, password: &str) -> eyre::Result<()> {
        validate_password_length(
            password,
            self.config.min_password_length,
            self.config.max_password_length,
        )
    }

    /// Re-key a root key stored under the legacy unsalted-SHA256 id onto the
    /// salted-KDF id, in place, on a successful credential match.
    ///
    /// Returns the key under its NEW id when the migration applies, `None` when
    /// no legacy key exists for these credentials (i.e. the credentials are
    /// simply wrong). The move is write-then-delete: if the process dies between
    /// the two, the key exists under both ids and the next login resolves via
    /// the new id — never a state where the key is gone.
    async fn migrate_legacy_key(
        &self,
        username: &str,
        password: &str,
        new_key_id: &str,
    ) -> eyre::Result<Option<(String, Key)>> {
        let legacy_id = legacy_key_id(username, password);

        let Some(key) = self.key_manager.get_key(&legacy_id).await? else {
            return Ok(None);
        };
        if !key.is_valid() || !key.is_root_key() {
            return Ok(None);
        }

        // Store under the new id first so a crash can't lose the key.
        self.key_manager.set_key(new_key_id, &key).await?;
        if let Err(e) = self.key_manager.delete_key(&legacy_id).await {
            // The new id is already usable; a stale legacy copy is harmless and
            // will be retried on the next login. Don't fail the login for it.
            warn!(
                error = %e,
                "Migrated user_password key id but failed to delete the legacy entry"
            );
        }

        info!(
            user = %crate::utils::sanitize_for_log(username),
            "Migrated user_password root key from the legacy unsalted key id to the salted KDF id"
        );

        Ok(Some((new_key_id.to_string(), key)))
    }

    /// Verify username and password by checking if corresponding root key exists
    ///
    /// # Arguments
    ///
    /// * `username` - The username
    /// * `password` - The password
    ///
    /// # Returns
    ///
    /// * `eyre::Result<Option<(String, Key)>>` - The key ID and root key if valid
    async fn verify_credentials(
        &self,
        username: &str,
        password: &str,
    ) -> eyre::Result<Option<(String, Key)>> {
        // Generate key ID from username/password
        let key_id = self.generate_key_id(username, password);

        // Try to get the root key
        match self.key_manager.get_key(&key_id).await {
            Ok(Some(key)) => {
                if key.is_valid() && key.is_root_key() {
                    Ok(Some((key_id, key)))
                } else {
                    Ok(None)
                }
            }
            // No key under the salted id. Before rejecting, check whether this
            // node still stores the key under the OLD unsalted-SHA256 id: on an
            // upgraded node every pre-existing user is in exactly that state,
            // and without this migration they would be permanently locked out
            // of their own node (the login path never mints keys, so nothing
            // else could rescue them).
            Ok(None) => self.migrate_legacy_key(username, password, &key_id).await,
            Err(err) => {
                error!("Failed to get root key: {}", err);
                Err(eyre::eyre!("Failed to verify credentials: {}", err))
            }
        }
    }

    /// Core authentication logic for username/password
    ///
    /// # Arguments
    ///
    /// * `username` - The username
    /// * `password` - The password
    ///
    /// # Returns
    ///
    /// * `eyre::Result<(String, Vec<String>)>` - The key ID and permissions
    async fn authenticate_core(
        &self,
        username: &str,
        password: &str,
    ) -> eyre::Result<(String, Vec<String>)> {
        // On the authentication path enforce only the MAXIMUM length (a bound on
        // PBKDF2 work per request). The minimum is a policy for new credentials
        // and is enforced on the creation paths (`crate::provisioning` and the
        // trait-level `create_root_key`): applying it here would reject an
        // existing user whose password predates the policy, locking them out of
        // their own node with no recovery path.
        validate_password_for_auth(password, self.config.max_password_length)?;

        // Try to verify existing credentials
        if let Some((key_id, root_key)) = self.verify_credentials(username, password).await? {
            // Existing user - return their key ID and permissions
            let permissions = root_key.permissions.clone();
            debug!(
                user = %crate::utils::sanitize_for_log(username),
                ?permissions,
                "Existing user authenticated"
            );
            return Ok((key_id, permissions));
        }

        // No credential match. There is deliberately no first-login bootstrap
        // branch here: the login path can never mint a root key. The admin
        // account is provisioned out of band — at `merod init`, at startup
        // from operator-supplied environment credentials, or offline via
        // `merod auth set-admin` (see `crate::provisioning`). The error is the
        // same generic message whether the node has no accounts at all or the
        // credentials are simply wrong, so a probe cannot distinguish the two.
        debug!("Authentication rejected: no matching root key");
        Err(eyre::eyre!("Invalid username or password"))
    }
}

/// Username/password auth verifier
struct UserPasswordVerifier {
    provider: Arc<UserPasswordProvider>,
    auth_data: UserPasswordAuthData,
}

#[async_trait]
impl AuthVerifierFn for UserPasswordVerifier {
    async fn verify(&self) -> eyre::Result<AuthResponse> {
        let auth_data = &self.auth_data;

        // Authenticate using the core authentication logic
        let (key_id, permissions) = self
            .provider
            .authenticate_core(&auth_data.username, &auth_data.password)
            .await?;

        // Return the authentication response
        Ok(AuthResponse {
            is_valid: true,
            key_id,
            permissions,
        })
    }
}

// Implement Clone for UserPasswordProvider
impl Clone for UserPasswordProvider {
    fn clone(&self) -> Self {
        Self {
            storage: Arc::clone(&self.storage),
            key_manager: self.key_manager.clone(),
            token_manager: self.token_manager.clone(),
            config: self.config.clone(),
        }
    }
}

/// Username/password specific request data
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct UserPasswordRequest {
    /// Username
    #[validate(length(min = 1, message = "Username is required"))]
    pub username: String,

    /// Password
    #[validate(length(min = 1, message = "Password is required"))]
    pub password: String,
}

#[async_trait]
impl AuthProvider for UserPasswordProvider {
    fn name(&self) -> &str {
        "user_password"
    }

    fn provider_type(&self) -> &str {
        "credentials"
    }

    fn description(&self) -> &str {
        "Authenticates users with username and password credentials"
    }

    fn supports_method(&self, method: &str) -> bool {
        method == "user_password" || method == "username_password"
    }

    fn is_configured(&self) -> bool {
        // Username/password provider is always technically configured (no external dependencies)
        true
    }

    async fn is_configured_with_users(&self) -> eyre::Result<bool> {
        // For username/password, "configured" means having users
        // Check if any root keys exist for this provider (auth_method = "user_password" or "username_password")
        use crate::storage::models::KeyType;
        self.key_manager
            .has_any_key(KeyType::Root, Some(&["user_password", "username_password"]))
            .await
            .map_err(|e| eyre::eyre!("Failed to check for user/password keys: {}", e))
    }

    fn get_config_options(&self) -> serde_json::Value {
        serde_json::json!({
            "enabled": true,
            "description": "Username and password authentication"
        })
    }

    fn prepare_auth_data(&self, token_request: &TokenRequest) -> eyre::Result<Value> {
        // Parse the provider-specific data into our request type
        let user_pass_data: UserPasswordRequest =
            serde_json::from_value(token_request.provider_data.clone())
                .map_err(|e| eyre::eyre!("Invalid username/password data: {}", e))?;

        // Create username/password specific auth data JSON
        Ok(serde_json::json!({
            "username": user_pass_data.username,
            "password": user_pass_data.password
        }))
    }

    fn create_verifier(
        &self,
        method: &str,
        auth_data: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<AuthRequestVerifier> {
        // Only handle supported methods
        if !self.supports_method(method) {
            return Err(eyre::eyre!(
                "Provider {} does not support method {}",
                self.name(),
                method
            ));
        }

        // Downcast to UserPasswordAuthData
        let user_pass_auth_data = auth_data
            .downcast_ref::<UserPasswordAuthData>()
            .ok_or_else(|| eyre::eyre!("Failed to parse username/password auth data"))?;

        // Create a clone of the auth data and provider for the verifier
        let auth_data_clone = user_pass_auth_data.clone();
        let provider = Arc::new(self.clone());

        // Create and return the verifier
        let verifier = UserPasswordVerifier {
            provider,
            auth_data: auth_data_clone,
        };

        Ok(AuthRequestVerifier::new(verifier))
    }

    fn verify_request(&self, request: &Request<Body>) -> eyre::Result<AuthRequestVerifier> {
        let headers = request.headers();

        // Extract username and password from headers
        let username = headers
            .get("x-username")
            .ok_or_else(|| eyre::eyre!("Missing username"))?
            .to_str()
            .map_err(|_| eyre::eyre!("Invalid username"))?
            .to_string();

        let password = headers
            .get("x-password")
            .ok_or_else(|| eyre::eyre!("Missing password"))?
            .to_str()
            .map_err(|_| eyre::eyre!("Invalid password"))?
            .to_string();

        // Create auth data
        let auth_data = UserPasswordAuthData { username, password };

        // Create verifier
        let provider = Arc::new(self.clone());
        let verifier = UserPasswordVerifier {
            provider,
            auth_data,
        };

        Ok(AuthRequestVerifier::new(verifier))
    }

    fn get_health_status(&self) -> eyre::Result<serde_json::Value> {
        Ok(serde_json::json!({
            "name": self.name(),
            "type": self.provider_type(),
            "configured": self.is_configured(),
        }))
    }

    async fn create_root_key(
        &self,
        public_key: &str,
        auth_method: &str,
        provider_data: Value,
        node_url: Option<&str>,
    ) -> eyre::Result<bool> {
        let username = provider_data
            .get("username")
            .and_then(Value::as_str)
            .ok_or_else(|| eyre::eyre!("Missing or invalid 'username' in provider data"))?;
        let password = provider_data
            .get("password")
            .and_then(Value::as_str)
            .ok_or_else(|| eyre::eyre!("Missing or invalid 'password' in provider data"))?;

        // Enforce password length bounds before creating the root key.
        self.validate_password(password)?;

        // Generate key ID from username/password
        let key_id = self.generate_key_id(username, password);

        // Create the root key
        let root_key = Key::new_root_key_with_permissions(
            public_key.to_string(),
            auth_method.to_string(),
            vec!["admin".to_string()],
            node_url.map(|s| s.to_string()),
        );

        // Store the root key using KeyManager
        let was_updated = self
            .key_manager
            .set_key(&key_id, &root_key)
            .await
            .map_err(|err| eyre::eyre!("Failed to store root key: {}", err))?;

        Ok(was_updated)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Username/Password provider registration
pub struct UserPasswordProviderRegistration;

impl ProviderRegistration for UserPasswordProviderRegistration {
    fn provider_id(&self) -> &str {
        "user_password"
    }

    fn create_provider(
        &self,
        context: ProviderContext,
    ) -> Result<Box<dyn AuthProvider>, eyre::Error> {
        let config = context.config.user_password.clone();
        let provider = UserPasswordProvider::new(context, config);
        Ok(Box::new(provider))
    }

    fn is_enabled(&self, config: &AuthConfig) -> bool {
        // Check if this provider is enabled in the config
        config
            .providers
            .get("user_password")
            .copied()
            .unwrap_or(false)
    }
}

// Register the username/password provider
register_auth_provider!(UserPasswordProviderRegistration);

// Register the username/password auth data type
register_auth_data_type!(UserPasswordAuthDataType);

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::config::JwtConfig;
    use crate::secrets::SecretManager;
    use crate::storage::models::KeyType;
    use crate::storage::MemoryStorage;

    /// A provider backed by in-memory storage. `config` lets a test set the
    /// bootstrap secret and length policy; pass `UserPasswordConfig::default()`
    /// for the default (min 8 / max 128, bootstrap disabled).
    fn test_provider(config: UserPasswordConfig) -> UserPasswordProvider {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let secret_manager = Arc::new(SecretManager::new(Arc::clone(&storage)));
        let token_manager = TokenManager::new(
            JwtConfig {
                issuer: "calimero-test".to_string(),
                access_token_expiry: 3600,
                refresh_token_expiry: 30 * 24 * 3600,
                node_host: None,
            },
            Arc::clone(&storage),
            secret_manager,
        );
        UserPasswordProvider {
            storage: Arc::clone(&storage),
            key_manager: KeyManager::new(storage),
            token_manager,
            config,
        }
    }

    async fn root_key_count(provider: &UserPasswordProvider) -> usize {
        provider
            .key_manager
            .list_keys(KeyType::Root)
            .await
            .unwrap()
            .len()
    }

    fn old_unsalted_key_id(username: &str, password: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!("user_password:{username}:{password}").as_bytes());
        hex::encode(hasher.finalize())
    }

    // --- the login path can never mint a key (TOFU removal, finding #2) --

    #[tokio::test]
    async fn first_login_on_fresh_node_never_mints_a_key() {
        // A fresh node has no root keys, and there is no bootstrap branch:
        // login must fail closed with the generic error, minting nothing.
        let provider = test_provider(UserPasswordConfig::default());
        let err = provider
            .authenticate_core("admin", "correct horse battery staple")
            .await
            .expect_err("first login on an unprovisioned node must be rejected");
        assert!(
            err.to_string().contains("Invalid username or password"),
            "rejection must be the generic credentials error, got: {err}"
        );
        assert_eq!(
            root_key_count(&provider).await,
            0,
            "the login path must never mint a root key"
        );
    }

    #[tokio::test]
    async fn provisioned_admin_key_authenticates() {
        // The out-of-band provisioning path (merod init / auth set-admin /
        // startup env credentials) mints the key; login then succeeds as a
        // plain existing-user authentication.
        let provider = test_provider(UserPasswordConfig::default());
        let key_id = crate::provisioning::provision_admin_key(
            &provider.storage,
            &provider.config,
            "admin",
            "password-1",
        )
        .await
        .expect("provisioning the admin key must succeed");

        let (login_id, perms) = provider
            .authenticate_core("admin", "password-1")
            .await
            .expect("provisioned admin must authenticate");
        assert_eq!(login_id, key_id);
        assert!(perms.contains(&"admin".to_string()));
        assert_eq!(root_key_count(&provider).await, 1);

        // A different identity still cannot log in — and cannot mint anything.
        assert!(provider
            .authenticate_core("intruder", "password-2")
            .await
            .is_err());
        assert_eq!(root_key_count(&provider).await, 1);
    }

    // --- legacy key-id migration (upgrade path, finding #4) --------------

    #[tokio::test]
    async fn legacy_key_id_is_migrated_in_place_on_login() {
        // An upgraded node stores the root key under the OLD unsalted id.
        // Without migration the salted lookup misses, root keys exist so the
        // bootstrap branch is skipped, and the operator is locked out forever.
        let provider = test_provider(UserPasswordConfig::default());
        let (user, pass) = ("alice", "correct horse battery staple");

        let legacy_id = old_unsalted_key_id(user, pass);
        let key = Key::new_root_key_with_permissions(
            user.to_string(),
            "user_password".to_string(),
            vec!["admin".to_string()],
            None,
        );
        provider
            .key_manager
            .set_key(&legacy_id, &key)
            .await
            .unwrap();

        // The existing operator can still log in (no bootstrap secret needed —
        // they match on the migration/fast path, not the bootstrap branch).
        let (key_id, permissions) = provider
            .authenticate_core(user, pass)
            .await
            .expect("an existing user must not be locked out by the key-id change");

        // ...and they are silently re-keyed onto the salted id.
        let new_id = derive_key_id(user, pass);
        assert_eq!(key_id, new_id, "login must return the migrated key id");
        assert_eq!(permissions, vec!["admin".to_string()]);
        assert!(
            provider
                .key_manager
                .get_key(&new_id)
                .await
                .unwrap()
                .is_some(),
            "key must now exist under the salted id"
        );
        assert!(
            provider
                .key_manager
                .get_key(&legacy_id)
                .await
                .unwrap()
                .is_none(),
            "legacy entry must be removed after migration"
        );

        // A second login resolves directly via the new id.
        let (again, _) = provider.authenticate_core(user, pass).await.unwrap();
        assert_eq!(again, new_id);
    }

    #[tokio::test]
    async fn wrong_password_does_not_migrate_or_authenticate() {
        let provider = test_provider(UserPasswordConfig::default());
        let legacy_id = old_unsalted_key_id("alice", "right-password");
        let key = Key::new_root_key_with_permissions(
            "alice".to_string(),
            "user_password".to_string(),
            vec!["admin".to_string()],
            None,
        );
        provider
            .key_manager
            .set_key(&legacy_id, &key)
            .await
            .unwrap();

        // Wrong password: no legacy key exists for THOSE credentials.
        assert!(provider
            .authenticate_core("alice", "wrong-password")
            .await
            .is_err());
        // The real key is untouched.
        assert!(provider
            .key_manager
            .get_key(&legacy_id)
            .await
            .unwrap()
            .is_some());
    }

    // --- password bounds apply to CREATION, not to existing logins -------

    #[tokio::test]
    async fn short_legacy_password_still_authenticates_after_upgrade() {
        // The min-length policy must not lock out a user whose password predates
        // it (e.g. the `dev`/`dev` credentials every e2e harness uses).
        let provider = test_provider(UserPasswordConfig::default());
        let (user, pass) = ("dev", "dev"); // 3 chars, below the min of 8
        let legacy_id = old_unsalted_key_id(user, pass);
        let key = Key::new_root_key_with_permissions(
            user.to_string(),
            "user_password".to_string(),
            vec!["admin".to_string()],
            None,
        );
        provider
            .key_manager
            .set_key(&legacy_id, &key)
            .await
            .unwrap();

        assert!(
            provider.authenticate_core(user, pass).await.is_ok(),
            "an existing short password must still authenticate"
        );
    }

    #[tokio::test]
    async fn overlong_password_is_rejected_on_the_auth_path() {
        // The maximum IS enforced at login: it bounds PBKDF2 work per request.
        // (Creation-path min-length enforcement is covered in
        // `crate::provisioning`, where NEW credentials are minted.)
        let provider = test_provider(UserPasswordConfig::default());
        let err = provider
            .authenticate_core("alice", &"x".repeat(129))
            .await
            .expect_err("an over-long password must be rejected before the KDF runs");
        assert!(
            err.to_string().contains("at most"),
            "expected a max-length error, got: {err}"
        );
    }

    // --- salted KDF key-id derivation (finding #4) ----------------------

    #[test]
    fn test_derive_key_id_is_deterministic() {
        let a = derive_key_id("alice", "correct horse battery staple");
        let b = derive_key_id("alice", "correct horse battery staple");
        assert_eq!(a, b);
        assert_eq!(a.len(), KEY_ID_LEN * 2); // hex of 32 bytes
    }

    #[test]
    fn test_derive_key_id_differs_by_password() {
        let a = derive_key_id("alice", "password-one");
        let b = derive_key_id("alice", "password-two");
        assert_ne!(a, b);
    }

    #[test]
    fn test_derive_key_id_differs_by_username() {
        // Same password, different user -> different id (per-user salt).
        let a = derive_key_id("alice", "shared-password");
        let b = derive_key_id("bob", "shared-password");
        assert_ne!(a, b);
    }

    #[test]
    fn test_derive_key_id_is_salted_not_plain_sha256() {
        // The new derivation must not equal the old unsalted SHA256.
        let username = "alice";
        let password = "correct horse battery staple";
        assert_ne!(
            derive_key_id(username, password),
            old_unsalted_key_id(username, password)
        );
    }

    // --- password length enforcement (finding #17) ----------------------

    #[test]
    fn test_password_too_short_rejected() {
        let err = validate_password_length("short", 8, 128).unwrap_err();
        assert!(err.to_string().contains("at least 8"));
    }

    #[test]
    fn test_password_too_long_rejected() {
        let pw = "x".repeat(129);
        let err = validate_password_length(&pw, 8, 128).unwrap_err();
        assert!(err.to_string().contains("at most 128"));
    }

    #[test]
    fn test_password_within_bounds_accepted() {
        assert!(validate_password_length("just-right-pw", 8, 128).is_ok());
    }

    #[test]
    fn test_password_length_boundaries_inclusive() {
        // Exactly min and exactly max are accepted.
        assert!(validate_password_length(&"x".repeat(8), 8, 128).is_ok());
        assert!(validate_password_length(&"x".repeat(128), 8, 128).is_ok());
    }

    #[test]
    fn test_password_length_boundaries_exclusive() {
        // Exactly min - 1 and exactly max + 1 are rejected.
        assert!(validate_password_length(&"x".repeat(7), 8, 128).is_err());
        assert!(validate_password_length(&"x".repeat(129), 8, 128).is_err());
    }

    #[test]
    fn test_password_length_counts_unicode_scalars() {
        // 8 multi-byte characters should count as length 8, not byte length.
        let pw = "áéíóúñçü"; // 8 chars, > 8 bytes
        assert_eq!(pw.chars().count(), 8);
        assert!(validate_password_length(pw, 8, 128).is_ok());
    }
}
