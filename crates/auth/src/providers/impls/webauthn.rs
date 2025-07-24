use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{debug, error};
use validator::Validate;
use webauthn_rs::prelude::*;
use uuid::Uuid;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use crate::api::handlers::auth::TokenRequest;
use crate::auth::token::TokenManager;
use crate::config::{AuthConfig, WebAuthnConfig};
use crate::providers::core::provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};
use crate::providers::core::provider_data_registry::AuthDataType;
use crate::providers::core::provider_registry::ProviderRegistration;
use crate::providers::ProviderContext;
use crate::storage::models::{Key, KeyType};
use crate::storage::{KeyManager, Storage};
use crate::{register_auth_data_type, register_auth_provider, AuthResponse};

/// WebAuthn authentication data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebAuthnAuthData {
    /// User identifier (email, username, etc.)
    pub user_id: String,
    /// Challenge token that was used
    pub challenge: String,
    /// WebAuthn authentication response (JSON from navigator.credentials.get())
    pub webauthn_response: String,
    /// Whether this is a registration request (for new users)
    pub is_registration: bool,
    /// User display name (only needed for registration)
    pub user_display_name: Option<String>,
}

/// WebAuthn auth data type
pub struct WebAuthnAuthDataType;

impl AuthDataType for WebAuthnAuthDataType {
    fn method_name(&self) -> &str {
        "webauthn"
    }

    fn parse_from_value(&self, value: Value) -> eyre::Result<Box<dyn std::any::Any + Send + Sync>> {
        // Try to deserialize as WebAuthnAuthData
        match serde_json::from_value::<WebAuthnAuthData>(value) {
            Ok(data) => Ok(Box::new(data)),
            Err(err) => Err(eyre::eyre!("Invalid WebAuthn auth data: {}", err)),
        }
    }

    fn get_sample_structure(&self) -> Value {
        serde_json::json!({
            "user_id": "user@example.com",
            "challenge": "challenge-token",
            "webauthn_response": "{\"id\":\"...\",\"rawId\":\"...\",\"response\":{...}}",
            "is_registration": false,
            "user_display_name": "John Doe"
        })
    }
}

/// WebAuthn authentication provider
pub struct WebAuthnProvider {
    storage: Arc<dyn Storage>,
    key_manager: KeyManager,
    token_manager: TokenManager,
    config: WebAuthnConfig,
    webauthn: Webauthn,
}

impl WebAuthnProvider {
    /// Create a new WebAuthn provider
    pub fn new(context: ProviderContext, config: WebAuthnConfig) -> eyre::Result<Self> {
        // Parse the first origin from config - WebAuthn builder expects a single origin
        let origin = if config.origins.is_empty() {
            return Err(eyre::eyre!("WebAuthn requires at least one origin"));
        } else {
            Url::parse(&config.origins[0])
                .map_err(|e| eyre::eyre!("Invalid WebAuthn origin URL: {}", e))?
        };

        // Create WebAuthn instance
        let webauthn = WebauthnBuilder::new(&config.rp_id, &origin)
            .map_err(|e| eyre::eyre!("Failed to create WebAuthn instance: {}", e))?
            .rp_name(&config.rp_name)
            .build()
            .map_err(|e| eyre::eyre!("Failed to build WebAuthn instance: {}", e))?;

        Ok(Self {
            storage: context.storage,
            key_manager: context.key_manager,
            token_manager: context.token_manager,
            config,
            webauthn,
        })
    }

    /// Generate a unique key ID for a user
    fn generate_key_id(&self, user_id: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!("webauthn:{}", user_id).as_bytes());
        let hash = hasher.finalize();
        hex::encode(hash)
    }

    /// Verify WebAuthn authentication (like near_wallet signature verification)
    async fn verify_webauthn_authentication(
        &self,
        user_id: &str,
        nonce: &str,
        webauthn_response: &str,
        stored_credentials: &[Passkey],
    ) -> eyre::Result<bool> {
        // Start authentication ceremony
        let (_request_challenge_response, authentication_state) = self.webauthn
            .start_passkey_authentication(stored_credentials)
            .map_err(|e| eyre::eyre!("Failed to start WebAuthn authentication: {:?}", e))?;

        // Parse the authentication response
        let auth_response: PublicKeyCredential = serde_json::from_str(webauthn_response)
            .map_err(|e| eyre::eyre!("Invalid WebAuthn authentication response: {}", e))?;

        // Verify that the client data JSON contains our challenge nonce
        let client_data_json = std::str::from_utf8(&auth_response.response.client_data_json)
            .map_err(|e| eyre::eyre!("Invalid client data JSON encoding: {}", e))?;
        
        let client_data: serde_json::Value = serde_json::from_str(client_data_json)
            .map_err(|e| eyre::eyre!("Invalid client data JSON: {}", e))?;
        
        let response_challenge = client_data.get("challenge")
            .and_then(|c| c.as_str())
            .ok_or_else(|| eyre::eyre!("Missing challenge in client data"))?;

        // Decode and verify challenge matches our nonce (like near_wallet)
        let decoded_challenge = URL_SAFE_NO_PAD.decode(response_challenge)
            .map_err(|e| eyre::eyre!("Invalid challenge encoding: {}", e))?;
        
        let expected_challenge = nonce.as_bytes();
        if decoded_challenge != expected_challenge {
            return Ok(false);
        }

        // Complete the authentication ceremony
        let authentication_result = self.webauthn
            .finish_passkey_authentication(&auth_response, &authentication_state)
            .map_err(|e| eyre::eyre!("WebAuthn authentication verification failed: {:?}", e))?;

        // Update credential counter (prevent replay attacks) 
        let updated_credentials: Vec<Passkey> = stored_credentials
            .iter()
            .map(|cred| {
                let mut updated_cred = cred.clone();
                if updated_cred.cred_id() == authentication_result.cred_id() {
                    updated_cred.update_credential(&authentication_result);
                }
                updated_cred
            })
            .collect();

        // Store updated credentials
        self.store_user_credentials(user_id, &updated_credentials).await?;

        Ok(true)
    }

    /// Get stored credentials for a user (for full WebAuthn verification)
    async fn get_user_credentials(&self, user_id: &str) -> eyre::Result<Vec<Passkey>> {
        let credentials_key = format!("webauthn:credentials:{}", user_id);
        
        match self.storage.get(&credentials_key).await {
            Ok(Some(data)) => {
                // Try to deserialize as full Passkey objects
                match serde_json::from_slice::<Vec<Passkey>>(&data) {
                    Ok(credentials) => Ok(credentials),
                    Err(_) => {
                        // If that fails, this might be a bootstrap user with simple credentials
                        // Return empty vec to indicate they need full credential setup
                        Ok(Vec::new())
                    }
                }
            }
            Ok(None) => Ok(Vec::new()),
            Err(e) => Err(eyre::eyre!("Failed to get user credentials: {}", e)),
        }
    }

    /// Store credentials for a user
    async fn store_user_credentials(&self, user_id: &str, credentials: &[Passkey]) -> eyre::Result<()> {
        let credentials_key = format!("webauthn:credentials:{}", user_id);
        let data = serde_json::to_vec(credentials)
            .map_err(|e| eyre::eyre!("Failed to serialize credentials: {}", e))?;
        
        self.storage.set(&credentials_key, &data).await
            .map_err(|e| eyre::eyre!("Failed to store credentials: {}", e))?;
        
        Ok(())
    }

    /// Get or create the root key for a user
    async fn get_root_key_for_user(&self, user_id: &str) -> eyre::Result<Option<(String, Key)>> {
        let key_id = self.generate_key_id(user_id);

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
                Err(eyre::eyre!("Failed to get root key: {}", err))
            }
        }
    }

    /// Create a root key for a user
    async fn create_root_key(&self, user_id: &str, public_key: &str) -> eyre::Result<(String, Key)> {
        let key_id = self.generate_key_id(user_id);

        let root_key = Key::new_root_key_with_permissions(
            public_key.to_string(),
            "webauthn".to_string(),
            vec!["admin".to_string()], // Default admin permission
        );

        self.key_manager
            .set_key(&key_id, &root_key)
            .await
            .map_err(|err| eyre::eyre!("Failed to store root key: {}", err))?;

        Ok((key_id, root_key))
    }



    /// Core authentication logic for WebAuthn (exactly like near_wallet pattern)
    async fn authenticate_core(
        &self,
        user_id: &str,
        challenge: &str,
        webauthn_response: &str,
    ) -> eyre::Result<(String, Vec<String>)> {
        // First verify that the challenge is a valid challenge token (like near_wallet)
        let challenge_claims = self.token_manager.verify_challenge(challenge).await?;
        debug!("Challenge verified successfully");

        // Get stored credentials for user
        let stored_credentials = self.get_user_credentials(user_id).await?;
        
        if stored_credentials.is_empty() {
            return Err(eyre::eyre!("No credentials found for user: {}", user_id));
        }

        // Verify WebAuthn authentication
        let authentication_valid = self
            .verify_webauthn_authentication(user_id, &challenge_claims.nonce, webauthn_response, &stored_credentials)
            .await?;

        if !authentication_valid {
            return Err(eyre::eyre!("WebAuthn authentication failed"));
        }
        debug!("WebAuthn authentication successful");

        // Get or create the root key (exactly like near_wallet)
        let (key_id, root_key) = match self.get_root_key_for_user(user_id).await? {
            Some((key_id, root_key)) => (key_id, root_key),
            None => {
                // Check if this is bootstrap case
                let existing_keys = self.key_manager.list_keys(KeyType::Root).await?;

                if existing_keys.is_empty() {
                    // Bootstrap: create first root key
                    let public_key = format!("webauthn:{}", user_id);
                    self.create_root_key(user_id, &public_key).await?
                } else {
                    // Root keys exist - this user is not authorized
                    return Err(eyre::eyre!("WebAuthn user {} is not authorized", user_id));
                }
            }
        };

        let permissions = root_key.permissions.clone();
        debug!(
            "Returning permissions for key {}: {:?}",
            key_id, permissions
        );

        Ok((key_id, permissions))
    }


}

/// WebAuthn auth verifier
struct WebAuthnVerifier {
    provider: Arc<WebAuthnProvider>,
    auth_data: WebAuthnAuthData,
}

#[async_trait]
impl AuthVerifierFn for WebAuthnVerifier {
    async fn verify(&self) -> eyre::Result<AuthResponse> {
        let auth_data = &self.auth_data;

        // Authenticate using the core authentication logic
        let (key_id, permissions) = self
            .provider
            .authenticate_core(
                &auth_data.user_id,
                &auth_data.challenge,
                &auth_data.webauthn_response,
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

// Implement Clone for WebAuthnProvider
impl Clone for WebAuthnProvider {
    fn clone(&self) -> Self {
        // Note: WebAuthn instance needs to be recreated since it doesn't implement Clone
        let context = ProviderContext {
            storage: Arc::clone(&self.storage),
            key_manager: self.key_manager.clone(),
            token_manager: self.token_manager.clone(),
            config: Arc::new(crate::config::AuthConfig {
                listen_addr: "127.0.0.1".parse().unwrap(),
                jwt: crate::config::JwtConfig {
                    issuer: "calimero-auth".to_string(),
                    access_token_expiry: 3600,
                    refresh_token_expiry: 2592000,
                },
                storage: crate::config::StorageConfig::Memory,
                cors: Default::default(),
                security: Default::default(),
                providers: Default::default(),
                near: Default::default(),
                user_password: Default::default(),
                webauthn: self.config.clone(),
            }),
        };
        
        Self::new(context, self.config.clone())
            .expect("Failed to clone WebAuthn provider")
    }
}

/// WebAuthn specific request data
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct WebAuthnRequest {
    /// User identifier
    #[validate(length(min = 1, message = "User ID is required"))]
    pub user_id: String,

    /// Challenge token
    #[validate(length(min = 1, message = "Challenge is required"))]
    pub challenge: String,

    /// WebAuthn authentication response (JSON from navigator.credentials.get())
    #[validate(length(min = 1, message = "WebAuthn response is required"))]
    pub webauthn_response: String,

    /// Whether this is a registration request (for new users)
    pub is_registration: bool,

    /// User display name (only needed for registration)
    pub user_display_name: Option<String>,
}

#[async_trait]
impl AuthProvider for WebAuthnProvider {
    fn name(&self) -> &str {
        "webauthn"
    }

    fn provider_type(&self) -> &str {
        "authenticator"
    }

    fn description(&self) -> &str {
        "Authenticates users with WebAuthn (FIDO2) authenticators"
    }

    fn supports_method(&self, method: &str) -> bool {
        method == "webauthn" || method == "fido2"
    }

    fn is_configured(&self) -> bool {
        !self.config.rp_id.is_empty() && !self.config.origins.is_empty()
    }

    fn get_config_options(&self) -> serde_json::Value {
        serde_json::json!({
            "rp_name": self.config.rp_name,
            "rp_id": self.config.rp_id,
            "origins": self.config.origins,
            "timeout": self.config.timeout,
            "user_verification": self.config.user_verification,
        })
    }

    fn prepare_auth_data(&self, token_request: &TokenRequest) -> eyre::Result<Value> {
        // Parse the provider-specific data into our request type
        let webauthn_data: WebAuthnRequest =
            serde_json::from_value(token_request.provider_data.clone())
                .map_err(|e| eyre::eyre!("Invalid WebAuthn data: {}", e))?;

        // Create WebAuthn-specific auth data JSON
        Ok(serde_json::json!({
            "user_id": webauthn_data.user_id,
            "challenge": webauthn_data.challenge,
            "webauthn_response": webauthn_data.webauthn_response,
            "is_registration": webauthn_data.is_registration,
            "user_display_name": webauthn_data.user_display_name,
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

        // Downcast to WebAuthnAuthData
        let webauthn_auth_data = auth_data
            .downcast_ref::<WebAuthnAuthData>()
            .ok_or_else(|| eyre::eyre!("Failed to parse WebAuthn auth data"))?;

        // Create a clone of the auth data and provider for the verifier
        let auth_data_clone = webauthn_auth_data.clone();
        let provider = Arc::new(self.clone());

        // Create and return the verifier
        let verifier = WebAuthnVerifier {
            provider,
            auth_data: auth_data_clone,
        };

        Ok(AuthRequestVerifier::new(verifier))
    }

    fn verify_request(&self, request: &Request<Body>) -> eyre::Result<AuthRequestVerifier> {
        let headers = request.headers();

        // Extract WebAuthn data from headers
        let user_id = headers
            .get("x-webauthn-user-id")
            .ok_or_else(|| eyre::eyre!("Missing WebAuthn user ID"))?
            .to_str()
            .map_err(|_| eyre::eyre!("Invalid WebAuthn user ID"))?
            .to_string();

        let challenge = headers
            .get("x-webauthn-challenge")
            .ok_or_else(|| eyre::eyre!("Missing WebAuthn challenge"))?
            .to_str()
            .map_err(|_| eyre::eyre!("Invalid WebAuthn challenge"))?
            .to_string();

        let webauthn_response = headers
            .get("x-webauthn-response")
            .ok_or_else(|| eyre::eyre!("Missing WebAuthn response"))?
            .to_str()
            .map_err(|_| eyre::eyre!("Invalid WebAuthn response"))?
            .to_string();

        let is_registration = headers
            .get("x-webauthn-is-registration")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(false);

        let user_display_name = headers
            .get("x-webauthn-user-display-name")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());

        // Create auth data
        let auth_data = WebAuthnAuthData {
            user_id,
            challenge,
            webauthn_response,
            is_registration,
            user_display_name,
        };

        // Create verifier
        let provider = Arc::new(self.clone());
        let verifier = WebAuthnVerifier {
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
            "rp_id": self.config.rp_id,
            "rp_name": self.config.rp_name,
            "origins": self.config.origins,
        }))
    }

    async fn create_root_key(
        &self,
        public_key: &str,
        auth_method: &str,
        provider_data: Value,
    ) -> eyre::Result<bool> {
        // Extract WebAuthn registration data
        let user_id = provider_data.get("user_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre::eyre!("Missing user_id in provider_data"))?;
        
        let webauthn_response = provider_data.get("webauthn_response")
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre::eyre!("Missing webauthn_response in provider_data"))?;

        // Check if user already has credentials
        let existing_credentials = self.get_user_credentials(user_id).await?;
        if !existing_credentials.is_empty() {
            return Err(eyre::eyre!("User {} already has WebAuthn credentials", user_id));
        }

        // Perform WebAuthn registration ceremony
        let user_uuid = Uuid::new_v4();
        let (_creation_challenge_response, registration_state) = self.webauthn
            .start_passkey_registration(
                user_uuid,
                user_id,
                user_id, // Use user_id as display name
                None,
            )
            .map_err(|e| eyre::eyre!("Failed to start WebAuthn registration: {:?}", e))?;

        // Parse the registration response
        let reg_response: RegisterPublicKeyCredential = serde_json::from_str(webauthn_response)
            .map_err(|e| eyre::eyre!("Invalid WebAuthn registration response: {}", e))?;

        // Complete the registration ceremony
        let passkey = self.webauthn
            .finish_passkey_registration(&reg_response, &registration_state)
            .map_err(|e| eyre::eyre!("WebAuthn registration verification failed: {:?}", e))?;

        debug!("WebAuthn registration ceremony completed successfully for user: {}", user_id);

        // Store the credential
        self.store_user_credentials(user_id, &[passkey.clone()]).await?;

        // Create the root key
        let key_id = self.generate_key_id(user_id);
        let root_key = Key::new_root_key_with_permissions(
            public_key.to_string(),
            auth_method.to_string(),
            vec!["admin".to_string()],
        );

        // Store the root key using KeyManager
        let was_updated = self
            .key_manager
            .set_key(&key_id, &root_key)
            .await
            .map_err(|err| eyre::eyre!("Failed to store root key: {}", err))?;

        debug!("WebAuthn root key created successfully for user: {}", user_id);
        Ok(was_updated)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// WebAuthn provider registration
pub struct WebAuthnProviderRegistration;

impl ProviderRegistration for WebAuthnProviderRegistration {
    fn provider_id(&self) -> &str {
        "webauthn"
    }

    fn create_provider(
        &self,
        context: ProviderContext,
    ) -> Result<Box<dyn AuthProvider>, eyre::Error> {
        let config = context.config.webauthn.clone();
        let provider = WebAuthnProvider::new(context, config)?;
        Ok(Box::new(provider))
    }

    fn is_enabled(&self, config: &AuthConfig) -> bool {
        // Check if this provider is enabled in the config
        config
            .providers
            .get("webauthn")
            .copied()
            .unwrap_or(false)
    }
}

// Register the WebAuthn provider
register_auth_provider!(WebAuthnProviderRegistration);

// Register the WebAuthn auth data type
register_auth_data_type!(WebAuthnAuthDataType); 