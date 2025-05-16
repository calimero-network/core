use std::any::Any;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use near_crypto::{KeyType, PublicKey, Signature};
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_primitives::types::{AccountId, BlockReference, Finality};
use near_primitives::views::QueryRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{debug, error};

use crate::api::handlers::auth::TokenRequest;
use crate::auth::token::TokenManager;
use crate::config::{AuthConfig, NearWalletConfig};
use crate::providers::core::provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};
use crate::providers::core::provider_data_registry::AuthDataType;
use crate::providers::core::provider_registry::ProviderRegistration;
use crate::storage::models::{prefixes, RootKey};
use crate::storage::{deserialize, serialize, KeyStorage};
use crate::{
    register_auth_data_type, register_auth_provider, AuthError, AuthResponse, RequestValidator,
};

/// NEAR wallet authentication data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NearWalletAuthData {
    /// Account ID of the NEAR wallet
    pub account_id: String,
    /// Public key of the NEAR wallet  
    pub public_key: String,
    /// Message to sign
    pub message: Vec<u8>,
    /// Signature of the message
    pub signature: String,
}

/// NEAR wallet auth data type
pub struct NearWalletAuthDataType;

impl AuthDataType for NearWalletAuthDataType {
    fn method_name(&self) -> &str {
        "near_wallet"
    }

    fn parse_from_value(
        &self,
        value: Value,
    ) -> Result<Box<dyn std::any::Any + Send + Sync>, AuthError> {
        // Try to deserialize as NearWalletAuthData
        match serde_json::from_value::<NearWalletAuthData>(value) {
            Ok(data) => Ok(Box::new(data)),
            Err(err) => Err(AuthError::InvalidRequest(format!(
                "Invalid NEAR wallet auth data: {}",
                err
            ))),
        }
    }

    fn get_sample_structure(&self) -> Value {
        serde_json::json!({
            "account_id": "example.near",
            "public_key": "ed25519:...",
            "message": "base64-encoded-message",
            "signature": "base64-encoded-signature",
        })
    }
}

/// NEAR wallet authentication provider
pub struct NearWalletProvider {
    config: NearWalletConfig,
    storage: Arc<dyn KeyStorage>,
    token_manager: TokenManager,
}

impl NearWalletProvider {
    /// Create a new NEAR wallet provider
    ///
    /// # Arguments
    ///
    /// * `config` - NEAR wallet configuration
    /// * `storage` - Storage backend
    /// * `token_manager` - JWT token manager
    pub fn new(
        config: NearWalletConfig,
        storage: Arc<dyn KeyStorage>,
        token_manager: TokenManager,
    ) -> Self {
        Self {
            config,
            storage,
            token_manager,
        }
    }

    /// Extract signature message from request headers
    ///
    /// # Arguments
    ///
    /// * `headers` - Request headers
    ///
    /// # Returns
    ///
    /// * `Result<(String, String, Vec<u8>), AuthError>` - Account ID, public key, and signature message
    fn extract_signature_data<B>(
        &self,
        request: &Request<B>,
    ) -> Result<(String, String, Vec<u8>), AuthError> {
        let headers = request.headers();

        // Extract the account ID
        let account_id = headers
            .get("x-near-account-id")
            .ok_or_else(|| AuthError::AuthenticationFailed("Missing NEAR account ID".to_string()))?
            .to_str()
            .map_err(|_| AuthError::AuthenticationFailed("Invalid NEAR account ID".to_string()))?
            .to_string();

        // Extract the public key
        let public_key = headers
            .get("x-near-public-key")
            .ok_or_else(|| AuthError::AuthenticationFailed("Missing NEAR public key".to_string()))?
            .to_str()
            .map_err(|_| AuthError::AuthenticationFailed("Invalid NEAR public key".to_string()))?
            .to_string();

        // Extract the signature
        let signature = headers
            .get("x-near-signature")
            .ok_or_else(|| AuthError::AuthenticationFailed("Missing NEAR signature".to_string()))?
            .to_str()
            .map_err(|_| AuthError::AuthenticationFailed("Invalid NEAR signature".to_string()))?;

        // Extract the message
        let message = headers
            .get("x-near-message")
            .ok_or_else(|| AuthError::AuthenticationFailed("Missing NEAR message".to_string()))?
            .to_str()
            .map_err(|_| AuthError::AuthenticationFailed("Invalid NEAR message".to_string()))?;

        // Verify the signature
        let message_bytes = message.as_bytes();
        let _signature_bytes = STANDARD.decode(signature).map_err(|_| {
            AuthError::AuthenticationFailed("Invalid NEAR signature encoding".to_string())
        })?;

        Ok((account_id, public_key, message_bytes.to_vec()))
    }

    /// Verify a NEAR signature
    ///
    /// # Arguments
    ///
    /// * `public_key` - The public key to verify with
    /// * `message` - The message that was signed
    /// * `signature` - The signature to verify
    ///
    /// # Returns
    ///
    /// * `Result<bool, AuthError>` - Whether the signature is valid
    async fn verify_signature(
        &self,
        public_key_str: &str,
        message: &[u8],
        signature_str: &str,
    ) -> Result<bool, AuthError> {
        // Validate inputs
        if public_key_str.is_empty() {
            return Err(AuthError::InvalidRequest(
                "Public key cannot be empty".to_string(),
            ));
        }

        if message.is_empty() {
            return Err(AuthError::InvalidRequest(
                "Message cannot be empty".to_string(),
            ));
        }

        if signature_str.is_empty() {
            return Err(AuthError::InvalidRequest(
                "Signature cannot be empty".to_string(),
            ));
        }

        // Parse the public key
        let public_key = PublicKey::from_str(public_key_str).map_err(|err| {
            AuthError::SignatureVerificationFailed(format!(
                "Invalid NEAR public key format: {}",
                err
            ))
        })?;

        // Parse the signature
        let signature_bytes = STANDARD.decode(signature_str).map_err(|err| {
            AuthError::SignatureVerificationFailed(format!(
                "Invalid NEAR signature encoding: {}",
                err
            ))
        })?;

        let signature =
            Signature::from_parts(KeyType::ED25519, &signature_bytes).map_err(|err| {
                AuthError::SignatureVerificationFailed(format!(
                    "Invalid NEAR signature format: {}",
                    err
                ))
            })?;

        // Verify the signature
        if !signature.verify(message, &public_key) {
            return Err(AuthError::SignatureVerificationFailed(
                "Signature verification failed".to_string(),
            ));
        }

        Ok(true)
    }

    /// Check if a public key belongs to a NEAR account
    ///
    /// # Arguments
    ///
    /// * `account_id` - The account ID to check
    /// * `public_key` - The public key to check
    ///
    /// # Returns
    ///
    /// * `Result<bool, AuthError>` - Whether the public key belongs to the account
    async fn verify_account_owns_key(
        &self,
        account_id: &str,
        public_key: &str,
    ) -> Result<bool, AuthError> {
        // Validate inputs
        if account_id.is_empty() {
            return Err(AuthError::InvalidRequest(
                "Account ID cannot be empty".to_string(),
            ));
        }

        if public_key.is_empty() {
            return Err(AuthError::InvalidRequest(
                "Public key cannot be empty".to_string(),
            ));
        }

        // Parse the account ID
        let account_id: AccountId = account_id.parse().map_err(|err| {
            AuthError::KeyOwnershipFailed(format!("Invalid NEAR account ID: {}", err))
        })?;

        // Parse the public key - use a variable first to avoid type annotation issues
        let public_key_result = public_key.parse::<near_crypto::PublicKey>();
        let parsed_public_key = public_key_result.map_err(|err| {
            AuthError::KeyOwnershipFailed(format!("Invalid NEAR public key format: {}", err))
        })?;

        // Connect to the NEAR RPC with retry logic
        let max_retries = 3;
        let mut attempt = 0;
        let mut last_error = None;

        while attempt < max_retries {
            attempt += 1;

            // Create a new client for each attempt
            let client = JsonRpcClient::connect(&self.config.rpc_url);

            // Query the account's access keys
            let request = methods::query::RpcQueryRequest {
                block_reference: BlockReference::Finality(Finality::Final),
                request: QueryRequest::ViewAccessKey {
                    account_id: account_id.clone(),
                    public_key: parsed_public_key.clone(),
                },
            };

            // Send the request
            match client.call(request).await {
                Ok(_) => return Ok(true), // If we get a valid response, the key belongs to the account
                Err(err) => {
                    debug!(
                        "Failed to verify NEAR account key (attempt {}/{}): {}",
                        attempt, max_retries, err
                    );
                    last_error = Some(err.to_string());

                    if attempt < max_retries {
                        // Wait before retrying with exponential backoff
                        let delay_ms = 100 * (2_u64.pow(attempt as u32));
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                }
            }
        }

        // If we're here, all retries failed
        if last_error.is_some() {
            debug!(
                "Final error verifying account key ownership: {}",
                last_error.unwrap()
            );
        }

        // If we get an error, it might be because the key doesn't exist
        // But it could also be a network issue, so we'll check for specific error patterns
        // For now, to be safe, we'll return false
        Ok(false)
    }

    /// Get the root key for an account
    ///
    /// # Arguments
    ///
    /// * `account_id` - The account ID to get the root key for
    ///
    /// # Returns
    ///
    /// * `Result<Option<(String, RootKey)>, AuthError>` - The root key ID and root key, if found
    async fn get_root_key_for_account(
        &self,
        account_id: &str,
    ) -> Result<Option<(String, RootKey)>, AuthError> {
        // Create a hash of the account ID to use as a lookup key
        let mut hasher = Sha256::new();
        hasher.update(format!("near:{account_id}").as_bytes());
        let hash = hasher.finalize();
        let key_id = hex::encode(hash);

        // Look up the root key
        let key = format!("{}{}", prefixes::ROOT_KEY, key_id);

        match self.storage.get(&key).await {
            Ok(Some(data)) => {
                let root_key: RootKey = deserialize(&data).map_err(|err| {
                    AuthError::StorageError(format!("Failed to deserialize root key: {}", err))
                })?;

                // Check if the key has been revoked
                if root_key.revoked_at.is_some() {
                    return Ok(None);
                }

                Ok(Some((key_id, root_key)))
            }
            Ok(None) => Ok(None),
            Err(err) => {
                error!("Failed to get root key: {}", err);
                Err(AuthError::StorageError(format!(
                    "Failed to get root key: {}",
                    err
                )))
            }
        }
    }

    /// Create a root key for an account
    ///
    /// # Arguments
    ///
    /// * `account_id` - The account ID to create a root key for
    /// * `public_key` - The public key to associate with the root key
    ///
    /// # Returns
    ///
    /// * `Result<(String, RootKey), AuthError>` - The created root key ID and root key
    async fn create_root_key(
        &self,
        account_id: &str,
        public_key: &str,
    ) -> Result<(String, RootKey), AuthError> {
        // Create a hash of the account ID to use as a key ID
        let mut hasher = Sha256::new();
        hasher.update(format!("near:{account_id}").as_bytes());
        let hash = hasher.finalize();
        let key_id = hex::encode(hash);

        // Create the root key
        let root_key = RootKey {
            public_key: public_key.to_string(),
            auth_method: "near_wallet".to_string(),
            created_at: Utc::now().timestamp() as u64,
            revoked_at: None,
            last_used_at: Some(Utc::now().timestamp() as u64),
            permissions: vec!["admin".to_string()], // Default admin permission
            metadata: None,
        };

        // Store the root key
        let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
        let data = serialize(&root_key).map_err(|err| {
            AuthError::StorageError(format!("Failed to serialize root key: {}", err))
        })?;

        self.storage
            .set(&key, &data)
            .await
            .map_err(|err| AuthError::StorageError(format!("Failed to store root key: {}", err)))?;

        Ok((key_id, root_key))
    }

    /// Update the last used timestamp for a root key
    ///
    /// # Arguments
    ///
    /// * `key_id` - The root key ID to update
    ///
    /// # Returns
    ///
    /// * `Result<(), AuthError>` - Success or error
    async fn update_last_used(&self, key_id: &str) -> Result<(), AuthError> {
        let key = format!("{}{}", prefixes::ROOT_KEY, key_id);

        match self.storage.get(&key).await {
            Ok(Some(data)) => {
                let mut root_key: RootKey = deserialize(&data).map_err(|err| {
                    AuthError::StorageError(format!("Failed to deserialize root key: {}", err))
                })?;

                // Update the last used timestamp
                root_key.last_used_at = Some(Utc::now().timestamp() as u64);

                // Store the updated root key
                let data = serialize(&root_key).map_err(|err| {
                    AuthError::StorageError(format!("Failed to serialize root key: {}", err))
                })?;

                self.storage.set(&key, &data).await.map_err(|err| {
                    AuthError::StorageError(format!("Failed to update root key: {}", err))
                })?;

                Ok(())
            }
            Ok(None) => Err(AuthError::StorageError("Root key not found".to_string())),
            Err(err) => {
                error!("Failed to get root key: {}", err);
                Err(AuthError::StorageError(format!(
                    "Failed to get root key: {}",
                    err
                )))
            }
        }
    }

    /// Core authentication logic for NEAR wallet
    ///
    /// This is the shared authentication logic used by both the request handler and verifier
    ///
    /// # Arguments
    ///
    /// * `account_id` - The NEAR account ID
    /// * `public_key` - The public key
    /// * `message` - The message that was signed
    /// * `signature` - The signature
    ///
    /// # Returns
    ///
    /// * `Result<(String, Vec<String>), AuthError>` - The key ID and permissions
    async fn authenticate_core(
        &self,
        account_id: &str,
        public_key: &str,
        message: &[u8],
        signature: &str,
    ) -> Result<(String, Vec<String>), AuthError> {
        // Verify the signature
        self.verify_signature(public_key, message, signature)
            .await?;

        // Verify the account owns the key
        if !self.verify_account_owns_key(account_id, public_key).await? {
            return Err(AuthError::AuthenticationFailed(
                "Public key does not belong to account".to_string(),
            ));
        }

        // Get or create the root key
        let (key_id, _root_key) = match self.get_root_key_for_account(account_id).await? {
            Some((key_id, root_key)) => {
                // Update the last used timestamp
                self.update_last_used(&key_id).await?;
                (key_id, root_key)
            }
            None => {
                // Create a new root key
                self.create_root_key(account_id, public_key).await?
            }
        };

        // For now, grant admin permissions to all NEAR wallets
        // In a real implementation, you would look up the permissions from storage
        let permissions = vec!["admin".to_string()];

        Ok((key_id, permissions))
    }

    /// Handle authentication and generate tokens
    ///
    /// # Arguments
    ///
    /// * `request` - The request to authenticate
    /// * `account_id` - The account ID
    /// * `public_key` - The public key
    /// * `message` - The message
    ///
    /// # Returns
    ///
    /// * `Result<AuthResponse, AuthError>` - Authentication response
    async fn authenticate<B>(
        &self,
        request: &Request<B>,
        account_id: &str,
        public_key: &str,
        message: &[u8],
    ) -> Result<AuthResponse, AuthError>
    where
        B: Send + Sync,
    {
        // Extract the signature from headers
        let signature = request
            .headers()
            .get("x-near-signature")
            .ok_or_else(|| AuthError::AuthenticationFailed("Missing NEAR signature".to_string()))?
            .to_str()
            .map_err(|_| AuthError::AuthenticationFailed("Invalid NEAR signature".to_string()))?;

        // Authenticate using the core authentication logic
        let (key_id, permissions) = self
            .authenticate_core(account_id, public_key, message, signature)
            .await?;

        // Generate a client ID and client key
        let client_id = format!("client_{}", Utc::now().timestamp());

        // Generate tokens for the client
        match self
            .token_manager
            .generate_token_pair(&client_id, &key_id, &permissions)
            .await
        {
            Ok((access_token, refresh_token)) => {
                // Store the token info in request extensions (as a HashMap) for later use
                if let Some(extensions) = request
                    .extensions()
                    .get::<std::collections::HashMap<String, String>>()
                {
                    let mut new_extensions = extensions.clone();
                    new_extensions.insert("access_token".to_string(), access_token);
                    new_extensions.insert("refresh_token".to_string(), refresh_token);
                    new_extensions.insert("client_id".to_string(), client_id);
                }
            }
            Err(err) => {
                error!("Failed to generate tokens: {}", err);
                // Return error since token generation is now required
                return Err(AuthError::TokenGenerationFailed(err.to_string()));
            }
        }

        Ok(AuthResponse {
            is_valid: true,
            key_id: Some(key_id),
            permissions,
        })
    }

    /// Get the token manager
    ///
    /// # Returns
    ///
    /// * `&TokenManager` - Reference to the token manager
    pub fn get_token_manager(&self) -> &TokenManager {
        &self.token_manager
    }
}

#[async_trait]
impl<B: Send + Sync> RequestValidator<B> for NearWalletProvider {
    async fn validate_request(&self, request: &Request<B>) -> Result<AuthResponse, AuthError> {
        // Extract the signature data
        let (account_id, public_key, message) = self.extract_signature_data(request)?;

        // Authenticate the request
        self.authenticate(request, &account_id, &public_key, &message)
            .await
    }
}

/// NEAR wallet auth verifier
struct NearWalletVerifier {
    provider: Arc<NearWalletProvider>,
    auth_data: NearWalletAuthData,
}

#[async_trait]
impl AuthVerifierFn for NearWalletVerifier {
    async fn verify(&self) -> Result<AuthResponse, AuthError> {
        let auth_data = &self.auth_data;

        // Authenticate using the core authentication logic
        let (key_id, permissions) = self
            .provider
            .authenticate_core(
                &auth_data.account_id,
                &auth_data.public_key,
                &auth_data.message,
                &auth_data.signature,
            )
            .await?;

        // Return the authentication response
        Ok(AuthResponse {
            is_valid: true,
            key_id: Some(key_id),
            permissions,
        })
    }
}

// Implement Clone for NearWalletProvider
impl Clone for NearWalletProvider {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            storage: Arc::clone(&self.storage),
            token_manager: self.token_manager.clone(),
        }
    }
}

impl AuthProvider for NearWalletProvider {
    fn name(&self) -> &str {
        "near_wallet"
    }

    fn provider_type(&self) -> &str {
        "wallet"
    }

    fn description(&self) -> &str {
        "Authenticates users with a NEAR wallet through cryptographic signatures"
    }

    fn supports_method(&self, method: &str) -> bool {
        method == "near_wallet" || method == "near"
    }

    fn is_configured(&self) -> bool {
        !self.config.rpc_url.is_empty()
    }

    fn get_config_options(&self) -> serde_json::Value {
        serde_json::json!({
            "rpc_url": self.config.rpc_url,
            "network": self.config.network,
            "wallet_url": self.config.wallet_url,
        })
    }

    fn prepare_auth_data(&self, token_request: &TokenRequest) -> Result<Value, AuthError> {
        // NEAR wallet specific validation
        let account_id = match &token_request.wallet_address {
            Some(addr) => addr.clone(),
            None => {
                return Err(AuthError::InvalidRequest(
                    "Missing wallet address for NEAR wallet authentication".to_string(),
                ));
            }
        };

        let message = match &token_request.message {
            Some(msg) => msg.as_bytes().to_vec(),
            None => {
                return Err(AuthError::InvalidRequest(
                    "Missing message for NEAR wallet authentication".to_string(),
                ));
            }
        };

        // Encode binary message as base64 for NEAR's requirements
        let encoded_message = base64::engine::general_purpose::STANDARD.encode(&message);

        // Create NEAR-specific auth data JSON
        Ok(serde_json::json!({
            "account_id": account_id,
            "public_key": token_request.public_key,
            "message": encoded_message,
            "signature": token_request.signature
        }))
    }

    fn create_verifier(
        &self,
        method: &str,
        auth_data: Box<dyn Any + Send + Sync>,
    ) -> Result<AuthRequestVerifier, AuthError> {
        // Only handle supported methods
        if !self.supports_method(method) {
            return Err(AuthError::InvalidRequest(format!(
                "Provider {} does not support method {}",
                self.name(),
                method
            )));
        }

        // Downcast to NearWalletAuthData
        let near_auth_data = auth_data
            .downcast_ref::<NearWalletAuthData>()
            .ok_or_else(|| {
                AuthError::InvalidRequest("Failed to parse NEAR wallet auth data".to_string())
            })?;

        // Create a clone of the auth data and provider for the verifier
        let auth_data_clone = near_auth_data.clone();
        let provider = Arc::new(self.clone());

        // Create and return the verifier
        let verifier = NearWalletVerifier {
            provider,
            auth_data: auth_data_clone,
        };

        Ok(AuthRequestVerifier::new(verifier))
    }

    fn verify_request(&self, request: &Request<Body>) -> eyre::Result<AuthRequestVerifier> {
        let headers = request.headers();

        // Extract all needed data from the request
        let account_id = headers
            .get("x-near-account-id")
            .ok_or_else(|| eyre::eyre!("Missing NEAR account ID"))?
            .to_str()
            .map_err(|_| eyre::eyre!("Invalid NEAR account ID"))?
            .to_string();

        let public_key = headers
            .get("x-near-public-key")
            .ok_or_else(|| eyre::eyre!("Missing NEAR public key"))?
            .to_str()
            .map_err(|_| eyre::eyre!("Invalid NEAR public key"))?
            .to_string();

        let signature = headers
            .get("x-near-signature")
            .ok_or_else(|| eyre::eyre!("Missing NEAR signature"))?
            .to_str()
            .map_err(|_| eyre::eyre!("Invalid NEAR signature"))?
            .to_string();

        let message = headers
            .get("x-near-message")
            .ok_or_else(|| eyre::eyre!("Missing NEAR message"))?
            .to_str()
            .map_err(|_| eyre::eyre!("Invalid NEAR message"))?
            .as_bytes()
            .to_vec();

        // Create auth data
        let auth_data = NearWalletAuthData {
            account_id,
            public_key,
            message,
            signature,
        };

        // Create verifier
        let provider = Arc::new(self.clone());
        let verifier = NearWalletVerifier {
            provider,
            auth_data,
        };

        Ok(AuthRequestVerifier::new(verifier))
    }

    fn get_health_status(&self) -> eyre::Result<serde_json::Value> {
        // Test the RPC connection to verify if the provider is healthy
        // We're just checking if we can create a connection, but not actually making the request
        let _client = JsonRpcClient::connect(&self.config.rpc_url);

        // We'll do a minimal check that doesn't require waiting for response
        // Just check if we can create a client and make a request
        Ok(serde_json::json!({
            "name": self.name(),
            "type": self.provider_type(),
            "configured": self.is_configured(),
            "connection_active": !self.config.rpc_url.is_empty(),
            "rpc_url": self.config.rpc_url,
            "network": self.config.network,
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Registration for the NEAR wallet provider
#[derive(Clone)]
pub struct NearWalletRegistration;

impl ProviderRegistration for NearWalletRegistration {
    fn provider_id(&self) -> &str {
        "near_wallet"
    }

    fn create_provider(
        &self,
        storage: Arc<dyn KeyStorage>,
        config: &AuthConfig,
    ) -> Result<Box<dyn AuthProvider>, eyre::Error> {
        let near_config = config.near.clone();
        let token_manager = TokenManager::new(config.jwt.clone(), storage.clone());
        let provider = NearWalletProvider::new(near_config, storage, token_manager);
        Ok(Box::new(provider))
    }

    fn is_enabled(&self, config: &AuthConfig) -> bool {
        // Check if this provider is enabled in the config
        config
            .providers
            .get("near_wallet")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }
}

// Self-register the provider
register_auth_provider!(NearWalletRegistration);

// Register the NEAR wallet auth data type
register_auth_data_type!(NearWalletAuthDataType);
