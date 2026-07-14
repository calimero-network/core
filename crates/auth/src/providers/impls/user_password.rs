use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tracing::{debug, error};
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

/// Username/password authentication data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPasswordAuthData {
    /// Username
    pub username: String,
    /// Password (will be hashed)
    pub password: String,
    /// Out-of-band bootstrap secret, only consulted when creating the very
    /// first root key on a fresh node (ignored for existing users).
    #[serde(default)]
    pub bootstrap_secret: Option<String>,
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
        let mut hasher = Sha256::new();
        hasher.update(format!("user_password:{username}:{password}").as_bytes());
        let hash = hasher.finalize();
        hex::encode(hash)
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
            Ok(None) => Ok(None),
            Err(err) => {
                error!("Failed to get root key: {}", err);
                Err(eyre::eyre!("Failed to verify credentials: {}", err))
            }
        }
    }

    /// Create a new root key for username/password authentication
    ///
    /// # Arguments
    ///
    /// * `username` - The username
    /// * `password` - The password
    ///
    /// # Returns
    ///
    /// * `eyre::Result<(String, Key)>` - The created key ID and root key
    async fn create_root_key(&self, username: &str, password: &str) -> eyre::Result<(String, Key)> {
        // Generate key ID from username/password
        let key_id = self.generate_key_id(username, password);

        // Create the root key with username as public key
        let root_key = Key::new_root_key_with_permissions(
            username.to_string(), // Use username as the "public key"
            "user_password".to_string(),
            vec!["admin".to_string()], // Default admin permission
            None,                      // No node_id for bootstrap keys
        );

        // Store the root key using KeyManager
        self.key_manager
            .set_key(&key_id, &root_key)
            .await
            .map_err(|err| eyre::eyre!("Failed to store root key: {}", err))?;

        Ok((key_id, root_key))
    }

    /// Resolve the effective bootstrap secret.
    ///
    /// The `MERO_AUTH_BOOTSTRAP_SECRET` environment variable takes precedence
    /// (the recommended out-of-band channel); the configured value is used as
    /// a fallback. Returns `None` when neither is set, which disables
    /// first-login bootstrap entirely. An empty string in either source is
    /// treated as unset — otherwise a blank env interpolation or
    /// `bootstrap_secret = ""` in config would let `""` match a caller that
    /// omitted the field (`unwrap_or_default`), silently re-enabling the
    /// unauthenticated TOFU bootstrap this gate exists to close.
    fn effective_bootstrap_secret(&self) -> Option<String> {
        std::env::var("MERO_AUTH_BOOTSTRAP_SECRET")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.config
                    .bootstrap_secret
                    .clone()
                    .filter(|s| !s.is_empty())
            })
    }

    /// Constant-time comparison of a presented bootstrap secret against the
    /// expected one.
    ///
    /// Both values are hashed to fixed-length digests first, so the comparison
    /// runs in time independent of the secret's length or content and does not
    /// leak the expected length.
    fn bootstrap_secret_matches(expected: &str, provided: &str) -> bool {
        let expected_digest = Sha256::digest(expected.as_bytes());
        let provided_digest = Sha256::digest(provided.as_bytes());
        expected_digest.ct_eq(&provided_digest).into()
    }

    /// Core authentication logic for username/password
    ///
    /// # Arguments
    ///
    /// * `username` - The username
    /// * `password` - The password
    /// * `bootstrap_secret` - The out-of-band secret presented by the caller,
    ///   only consulted on the first-root-key bootstrap path
    ///
    /// # Returns
    ///
    /// * `eyre::Result<(String, Vec<String>)>` - The key ID and permissions
    async fn authenticate_core(
        &self,
        username: &str,
        password: &str,
        bootstrap_secret: Option<&str>,
    ) -> eyre::Result<(String, Vec<String>)> {
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

        // Check if this is the bootstrap case (no root keys exist)
        let existing_keys = self
            .key_manager
            .list_keys(crate::storage::models::KeyType::Root)
            .await?;

        if existing_keys.is_empty() {
            // Bootstrap case - create the first root key, but only for a caller
            // that proves possession of the out-of-band bootstrap secret.
            // Without this gate the first unauthenticated caller to reach a
            // fresh node is silently granted a ROOT admin key (trust-on-first-
            // use). We deliberately return the same generic error on every
            // failure path so a probe cannot distinguish "bootstrap disabled",
            // "wrong secret", and "already bootstrapped".
            let Some(expected_secret) = self.effective_bootstrap_secret() else {
                debug!("Bootstrap rejected: no bootstrap secret configured (bootstrap disabled)");
                return Err(eyre::eyre!("Invalid username or password"));
            };

            let provided_secret = bootstrap_secret.unwrap_or_default();
            if !Self::bootstrap_secret_matches(&expected_secret, provided_secret) {
                debug!("Bootstrap rejected: bootstrap secret missing or mismatched");
                return Err(eyre::eyre!("Invalid username or password"));
            }

            let (key_id, root_key) = self.create_root_key(username, password).await?;
            debug!(
                user = %crate::utils::sanitize_for_log(username),
                "Bootstrap: created first root key"
            );
            Ok((key_id, root_key.permissions))
        } else {
            // Root keys exist but credentials are invalid
            Err(eyre::eyre!("Invalid username or password"))
        }
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
            .authenticate_core(
                &auth_data.username,
                &auth_data.password,
                auth_data.bootstrap_secret.as_deref(),
            )
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

    /// Optional out-of-band bootstrap secret, only used to create the first
    /// root key on a fresh node.
    #[serde(default)]
    pub bootstrap_secret: Option<String>,
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
            "password": user_pass_data.password,
            "bootstrap_secret": user_pass_data.bootstrap_secret
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

        // Optional out-of-band bootstrap secret header.
        let bootstrap_secret = headers
            .get("x-bootstrap-secret")
            .and_then(|h| h.to_str().ok())
            .map(str::to_string);

        // Create auth data
        let auth_data = UserPasswordAuthData {
            username,
            password,
            bootstrap_secret,
        };

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
    use super::*;
    use crate::config::JwtConfig;
    use crate::secrets::SecretManager;
    use crate::storage::models::KeyType;
    use crate::storage::providers::memory::MemoryStorage;

    fn test_provider(config: UserPasswordConfig) -> UserPasswordProvider {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let secret_manager = Arc::new(SecretManager::new(Arc::clone(&storage)));
        let token_manager = TokenManager::new(
            JwtConfig {
                issuer: "test".to_string(),
                access_token_expiry: 3600,
                refresh_token_expiry: 30 * 24 * 3600,
            },
            Arc::clone(&storage),
            secret_manager,
        );
        let key_manager = KeyManager::new(Arc::clone(&storage));
        UserPasswordProvider {
            storage,
            key_manager,
            token_manager,
            config,
        }
    }

    fn config_with_secret(secret: Option<&str>) -> UserPasswordConfig {
        UserPasswordConfig {
            bootstrap_secret: secret.map(str::to_string),
            ..UserPasswordConfig::default()
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

    #[tokio::test]
    async fn bootstrap_disabled_by_default_rejects_first_login() {
        let provider = test_provider(config_with_secret(None));
        let result = provider
            .authenticate_core("admin", "correct horse battery staple", None)
            .await;
        assert!(
            result.is_err(),
            "bootstrap must fail closed when no bootstrap secret is configured"
        );
        assert_eq!(
            root_key_count(&provider).await,
            0,
            "no root key should be minted without a bootstrap secret"
        );
    }

    #[tokio::test]
    async fn bootstrap_requires_matching_secret() {
        let provider = test_provider(config_with_secret(Some("s3cr3t-bootstrap")));

        // Missing secret -> rejected, no key created.
        assert!(provider
            .authenticate_core("admin", "pw", None)
            .await
            .is_err());
        assert_eq!(root_key_count(&provider).await, 0);

        // Wrong secret -> rejected, no key created.
        assert!(provider
            .authenticate_core("admin", "pw", Some("wrong"))
            .await
            .is_err());
        assert_eq!(root_key_count(&provider).await, 0);

        // Correct secret -> exactly one admin root key minted.
        let (_, perms) = provider
            .authenticate_core("admin", "pw", Some("s3cr3t-bootstrap"))
            .await
            .expect("correct bootstrap secret should succeed");
        assert!(perms.contains(&"admin".to_string()));
        assert_eq!(root_key_count(&provider).await, 1);
    }

    #[tokio::test]
    async fn existing_user_authenticates_without_bootstrap_secret() {
        let provider = test_provider(config_with_secret(Some("s3cr3t-bootstrap")));

        // Bootstrap the first key with the secret.
        provider
            .authenticate_core("admin", "pw", Some("s3cr3t-bootstrap"))
            .await
            .unwrap();
        assert_eq!(root_key_count(&provider).await, 1);

        // The now-existing user authenticates on the fast path, no secret needed.
        let (_, perms) = provider
            .authenticate_core("admin", "pw", None)
            .await
            .expect("existing user should authenticate without the bootstrap secret");
        assert!(perms.contains(&"admin".to_string()));
        assert_eq!(root_key_count(&provider).await, 1, "no duplicate root key");

        // Once a root key exists, a different identity cannot bootstrap a second
        // one even with the correct secret.
        assert!(provider
            .authenticate_core("intruder", "pw2", Some("s3cr3t-bootstrap"))
            .await
            .is_err());
        assert_eq!(root_key_count(&provider).await, 1);
    }

    #[tokio::test]
    async fn empty_config_secret_is_treated_as_unset() {
        // `bootstrap_secret = ""` (e.g. a blank env interpolation in config)
        // must behave exactly like no secret at all: bootstrap stays disabled
        // and, critically, a caller omitting the field (which the verifier
        // defaults to "") must not match SHA-256("") == SHA-256("").
        let provider = test_provider(config_with_secret(Some("")));

        assert!(provider
            .authenticate_core("admin", "pw", None)
            .await
            .is_err());
        assert!(provider
            .authenticate_core("admin", "pw", Some(""))
            .await
            .is_err());
        assert_eq!(
            root_key_count(&provider).await,
            0,
            "an empty configured secret must never mint a root key"
        );
    }

    #[test]
    fn bootstrap_secret_matches_is_exact() {
        assert!(UserPasswordProvider::bootstrap_secret_matches("abc", "abc"));
        assert!(!UserPasswordProvider::bootstrap_secret_matches(
            "abc", "abcd"
        ));
        assert!(!UserPasswordProvider::bootstrap_secret_matches("abc", ""));
        assert!(!UserPasswordProvider::bootstrap_secret_matches(
            "abc", "abC"
        ));
    }
}
