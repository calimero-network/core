use std::any::Any;
use std::num::NonZeroU32;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use rand::RngExt;
use ring::pbkdf2;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, error};
use uuid::Uuid;
use validator::Validate;

use crate::api::handlers::auth::TokenRequest;
use crate::auth::token::TokenManager;
use crate::config::{AuthConfig, UserPasswordConfig};
use crate::providers::core::provider::{
    AuthProvider, AuthRequestVerifier, AuthVerifierFn, LoginRejection,
};
use crate::providers::core::provider_data_registry::AuthDataType;
use crate::providers::core::provider_registry::ProviderRegistration;
use crate::providers::ProviderContext;
use crate::storage::models::{Key, PasswordLogin};
use crate::storage::{KeyManager, Storage, StorageError};
use crate::{register_auth_data_type, register_auth_provider, AuthResponse};

const PASSWORD_PBKDF2_ITERATIONS: NonZeroU32 = NonZeroU32::new(600_000).unwrap(); // PBKDF2-HMAC-SHA256 rounds, the OWASP minimum
const PASSWORD_SALT_LEN: usize = 16; // random salt bytes per stored password
const PASSWORD_HASH_LEN: usize = 32; // stored hash bytes
static REGISTER_LOCK: Mutex<()> = Mutex::const_new(()); // one registration at a time, so a username gets one key id

/// Hash `password` under `salt`.
fn hash_password(password: &str, salt: &[u8]) -> Vec<u8> {
    let mut hash = vec![0u8; PASSWORD_HASH_LEN];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        PASSWORD_PBKDF2_ITERATIONS,
        salt,
        password.as_bytes(),
        &mut hash,
    );
    hash
}

/// Refuses a login this node cannot check, without counting it as a wrong password.
fn verification_unavailable() -> eyre::Report {
    eyre::Report::new(LoginRejection::Unavailable(
        "Password verification is unavailable".to_owned(),
    ))
}

/// Whether `password` opens `login`. An unknown user is checked against a dummy
/// hash, so the answer takes as long and does not tell which usernames exist.
fn password_opens(login: Option<&PasswordLogin>, password: &str) -> bool {
    let dummy = ([0u8; PASSWORD_SALT_LEN], [0u8; PASSWORD_HASH_LEN]);
    let (salt, hash) = login.map_or((&dummy.0[..], &dummy.1[..]), |login| {
        (&login.salt[..], &login.hash[..])
    });
    let matches = pbkdf2::verify(
        pbkdf2::PBKDF2_HMAC_SHA256,
        PASSWORD_PBKDF2_ITERATIONS,
        salt,
        password.as_bytes(),
        hash,
    )
    .is_ok();
    matches && login.is_some()
}

/// Store `key` as the root key `username` opens with `password` and return its
/// id and whether it replaced an account. A new password gets a new id.
pub(crate) async fn register_root_key(
    key_manager: &KeyManager,
    username: &str,
    password: &str,
    key: &Key,
) -> eyre::Result<(String, bool)> {
    let _guard = REGISTER_LOCK.lock().await;
    let previous = key_manager.password_login(username).await?;

    let (checked, password) = (previous.clone(), password.to_owned());
    let (same_password, salt, hash) = tokio::task::spawn_blocking(move || {
        let salt: [u8; PASSWORD_SALT_LEN] = rand::rng().random();
        let hash = hash_password(&password, &salt);
        let same = checked.is_some_and(|login| password_opens(Some(&login), &password));
        (same, salt.to_vec(), hash)
    })
    .await
    .map_err(|err| eyre::eyre!("Password hashing task failed: {err}"))?;

    // The id survives only while its key is live: reusing a revoked one would
    // bring back every token minted under it.
    let live_id = match &previous {
        Some(login) if same_password => key_manager
            .get_key(&login.key_id)
            .await?
            .map(|_| login.key_id.clone()),
        _ => None,
    };
    let key_id = live_id.unwrap_or_else(|| Uuid::new_v4().to_string());

    // Retire the old key before writing the new one: a crash then locks the user
    // out until a re-run, but never leaves the old password's sessions alive.
    if let Some(stale) = previous.as_ref().filter(|login| login.key_id != key_id) {
        let _revoked = key_manager
            .delete_client_keys_for_root(&stale.key_id)
            .await?;
        match key_manager.delete_key(&stale.key_id).await {
            Ok(()) | Err(StorageError::NotFound) => {}
            Err(err) => return Err(err.into()),
        }
    }
    key_manager
        .set_password_login(
            username,
            &PasswordLogin {
                key_id: key_id.clone(),
                salt,
                hash,
            },
        )
        .await?;
    let _existed = key_manager.set_key(&key_id, key).await?;
    Ok((key_id, previous.is_some()))
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
    /// Bounds how many key derivations run at once, so a burst of logins
    /// cannot fill the blocking pool.
    kdf_permits: Arc<Semaphore>,
}

impl UserPasswordProvider {
    /// Create a new username/password provider
    pub fn new(context: ProviderContext, config: UserPasswordConfig) -> Self {
        Self {
            storage: context.storage,
            key_manager: context.key_manager,
            token_manager: context.token_manager,
            config,
            kdf_permits: Arc::new(Semaphore::new(
                std::thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get),
            )),
        }
    }

    /// Check `password` against `login` off the async workers, behind the KDF semaphore.
    async fn check_password(
        &self,
        login: Option<PasswordLogin>,
        password: &str,
    ) -> eyre::Result<bool> {
        let permit = Arc::clone(&self.kdf_permits)
            .acquire_owned()
            .await
            .map_err(|err| {
                error!("Key derivation permit unavailable: {err}");
                verification_unavailable()
            })?;
        let password = password.to_owned();
        // The permit moves into the task so it is held until the derivation ends.
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            password_opens(login.as_ref(), &password)
        })
        .await
        .map_err(|err| {
            error!("Key derivation task failed: {err}");
            verification_unavailable()
        })
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
        let login = self
            .key_manager
            .password_login(username)
            .await
            .map_err(|err| {
                error!("Failed to read the password login: {err}");
                verification_unavailable()
            })?;
        let key_id = login.as_ref().map(|login| login.key_id.clone());
        let opens = self.check_password(login, password).await?;
        let Some(key_id) = key_id.filter(|_| opens) else {
            return Ok(None);
        };

        match self.key_manager.get_key(&key_id).await {
            Ok(Some(key)) if key.is_root_key() => Ok(Some((key_id, key))),
            Ok(_) => Ok(None),
            Err(err) => {
                error!("Failed to get root key: {err}");
                Err(verification_unavailable())
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
        validate_password_for_auth(password, self.config.max_password_length)
            .map_err(|err| LoginRejection::Invalid(err.to_string()))?;

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
            // A password session identifies the node owner, who is not a device
            // of anybody's account. Naming one here would be inventing a fact
            // the provider cannot know.
            device: None,
            is_valid: true,
            user_id: key_id.clone(),
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
            kdf_permits: Arc::clone(&self.kdf_permits),
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

    fn throttle_identity(&self, token_request: &TokenRequest) -> String {
        // The account being guessed, not the caller-chosen public key.
        token_request
            .provider_data
            .get("username")
            .and_then(Value::as_str)
            .map_or_else(|| token_request.public_key.clone(), str::to_owned)
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

        let root_key = Key::new_root_key_with_permissions(
            public_key.to_string(),
            auth_method.to_string(),
            vec!["admin".to_string()],
            node_url.map(|s| s.to_string()),
        );

        // An existing username gets the new password; its old key and sessions are revoked.
        let (_key_id, was_updated) =
            register_root_key(&self.key_manager, username, password, &root_key)
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
    use super::*;
    use crate::config::JwtConfig;
    use crate::secrets::SecretManager;
    use crate::storage::models::{prefixes, KeyType};
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
                client_access_token_expiry: crate::config::default_client_access_token_expiry(),
                client_refresh_token_expiry: crate::config::default_client_refresh_token_expiry(),
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
            kdf_permits: Arc::new(Semaphore::new(2)),
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

    #[tokio::test]
    async fn a_wrong_password_is_refused() {
        let provider = test_provider(UserPasswordConfig::default());
        let _key_id = crate::provisioning::provision_admin_key(
            &provider.storage,
            &provider.config,
            "alice",
            "right-password",
        )
        .await
        .unwrap();

        assert!(provider
            .authenticate_core("alice", "wrong-password")
            .await
            .is_err());
        assert!(provider
            .authenticate_core("alice", "right-password")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn create_root_key_assigns_a_random_key_id() {
        let mut ids = Vec::new();
        for _ in 0..2 {
            let provider = test_provider(UserPasswordConfig::default());
            let _created = provider
                .create_root_key(
                    "pk",
                    "user_password",
                    serde_json::json!({ "username": "alice", "password": "password-1" }),
                    None,
                )
                .await
                .unwrap();
            let (key_id, _) = provider
                .authenticate_core("alice", "password-1")
                .await
                .unwrap();
            ids.push(key_id);
        }

        assert_ne!(
            ids[0], ids[1],
            "the same credentials must not give the same id"
        );
    }

    #[tokio::test]
    async fn create_root_key_with_a_new_password_retires_the_old_one() {
        let provider = test_provider(UserPasswordConfig::default());
        let mut replaced = Vec::new();
        for password in ["password-1", "password-2"] {
            replaced.push(
                provider
                    .create_root_key(
                        "pk",
                        "user_password",
                        serde_json::json!({ "username": "alice", "password": password }),
                        None,
                    )
                    .await
                    .unwrap(),
            );
        }

        assert_eq!(replaced, [false, true], "the second call replaces alice");

        assert!(provider
            .authenticate_core("alice", "password-1")
            .await
            .is_err());
        assert!(provider
            .authenticate_core("alice", "password-2")
            .await
            .is_ok());
        assert_eq!(root_key_count(&provider).await, 1);
    }

    #[tokio::test]
    async fn the_same_credentials_are_hashed_under_a_fresh_salt_each_time() {
        let provider = test_provider(UserPasswordConfig::default());
        let mut logins = Vec::new();
        for _ in 0..2 {
            let _key_id = crate::provisioning::provision_admin_key(
                &provider.storage,
                &provider.config,
                "alice",
                "shared-password",
            )
            .await
            .unwrap();
            logins.push(
                provider
                    .key_manager
                    .password_login("alice")
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }

        assert_ne!(logins[0].salt, logins[1].salt);
        assert_ne!(logins[0].hash, logins[1].hash);
    }

    #[tokio::test]
    async fn concurrent_registrations_of_one_user_agree_on_one_id() {
        let provider = test_provider(UserPasswordConfig::default());
        let register = || {
            crate::provisioning::provision_admin_key(
                &provider.storage,
                &provider.config,
                "alice",
                "password-1",
            )
        };

        let (a, b) = tokio::join!(register(), register());

        assert_eq!(a.unwrap(), b.unwrap());
        assert_eq!(root_key_count(&provider).await, 1);
    }

    #[tokio::test]
    async fn a_revoked_user_is_refused_with_the_right_password() {
        let provider = test_provider(UserPasswordConfig::default());
        let key_id = crate::provisioning::provision_admin_key(
            &provider.storage,
            &provider.config,
            "alice",
            "password-1",
        )
        .await
        .unwrap();
        let mut key = provider
            .key_manager
            .get_key(&key_id)
            .await
            .unwrap()
            .unwrap();
        key.revoke();
        let _ = provider.key_manager.set_key(&key_id, &key).await.unwrap();

        assert!(provider
            .authenticate_core("alice", "password-1")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn a_login_whose_key_is_gone_is_refused_until_reprovisioned() {
        let provider = test_provider(UserPasswordConfig::default());
        let provision = || {
            crate::provisioning::provision_admin_key(
                &provider.storage,
                &provider.config,
                "alice",
                "password-1",
            )
        };
        let key_id = provision().await.unwrap();
        provider.key_manager.delete_key(&key_id).await.unwrap();

        assert!(provider
            .authenticate_core("alice", "password-1")
            .await
            .is_err());

        let _ = provision().await.unwrap();
        assert!(provider
            .authenticate_core("alice", "password-1")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn an_unreadable_login_is_refused() {
        let provider = test_provider(UserPasswordConfig::default());
        let path = format!("{}alice", prefixes::PASSWORD_LOGIN);
        provider.storage.set(&path, b"not a login").await.unwrap();

        let err = provider
            .authenticate_core("alice", "password-1")
            .await
            .expect_err("must be refused");
        assert!(matches!(
            err.downcast_ref::<LoginRejection>(),
            Some(LoginRejection::Unavailable(_))
        ));
    }

    // --- password bounds apply to CREATION, not to existing logins -------

    #[tokio::test]
    async fn a_password_below_a_raised_minimum_still_authenticates() {
        // The min-length policy must not lock out a user whose password predates
        // it (e.g. the `dev`/`dev` credentials every e2e harness uses).
        let provider = test_provider(UserPasswordConfig::default());
        let lax = UserPasswordConfig {
            min_password_length: 0,
            ..UserPasswordConfig::default()
        };
        let _key_id =
            crate::provisioning::provision_admin_key(&provider.storage, &lax, "dev", "dev")
                .await
                .unwrap();

        assert!(
            provider.authenticate_core("dev", "dev").await.is_ok(),
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

    #[tokio::test]
    async fn an_over_long_password_is_not_a_credential_failure() {
        let provider = test_provider(UserPasswordConfig::default());
        let err = provider
            .authenticate_core("alice", &"x".repeat(129))
            .await
            .expect_err("must be rejected");
        assert!(matches!(
            err.downcast_ref::<LoginRejection>(),
            Some(LoginRejection::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn unavailable_key_derivation_is_not_a_credential_failure() {
        let provider = test_provider(UserPasswordConfig::default());
        provider.kdf_permits.close();

        // No "alice" exists, so this also pins that an unknown user pays for a hash.
        let err = provider
            .authenticate_core("alice", "some password")
            .await
            .expect_err("must be refused");
        assert!(matches!(
            err.downcast_ref::<LoginRejection>(),
            Some(LoginRejection::Unavailable(_))
        ));
        assert_eq!(err.to_string(), "Password verification is unavailable");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn password_hashing_does_not_hold_the_async_worker() {
        // On one thread the second future runs only while the first is
        // pending, so it sees the flag unset only if the KDF ran off-thread.
        let provider = test_provider(UserPasswordConfig::default());
        let done = std::sync::atomic::AtomicBool::new(false);

        let (_, done_when_others_ran) = tokio::join!(
            async {
                let _ = provider.authenticate_core("alice", "some password").await;
                done.store(true, std::sync::atomic::Ordering::SeqCst);
            },
            async { done.load(std::sync::atomic::Ordering::SeqCst) },
        );

        assert!(
            !done_when_others_ran,
            "other tasks must be able to run while a password is being hashed"
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
