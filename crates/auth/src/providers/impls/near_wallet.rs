use std::any::Any;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use borsh::BorshSerialize;
use near_crypto::{KeyType as NearKeyType, PublicKey, Signature};
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_primitives::types::{AccountId, BlockReference, Finality};
use near_primitives::views::QueryRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{debug, error};
use validator::Validate;

use crate::api::handlers::auth::TokenRequest;
use crate::auth::token::TokenManager;
use crate::config::{AuthConfig, NearWalletConfig};
use crate::providers::core::provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};
use crate::providers::core::provider_data_registry::AuthDataType;
use crate::providers::core::provider_registry::ProviderRegistration;
use crate::providers::ProviderContext;
use crate::storage::models::Key;
use crate::storage::{KeyManager, Storage};
use crate::{register_auth_data_type, register_auth_provider, AuthResponse};

/// Represents the payload structure that contains a message, nonce, recipient, and optional callback URL.
///
/// # Fields
/// * `tag` - A tag to identify the payload type.
/// * `message` - The message to be sent.
/// * `nonce` - A 32-byte nonce for the message.
/// * `recipient` - The recipient of the message.
/// * `callback_url` - An optional callback URL for the message.
#[derive(BorshSerialize)]
struct Payload {
    tag: u32,
    message: String,
    nonce: [u8; 32],
    recipient: String,
    callback_url: Option<String>,
}

/// NEAR wallet authentication data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NearWalletAuthData {
    /// Account ID of the NEAR wallet
    pub account_id: String,
    /// Public key of the NEAR wallet  
    pub public_key: String,
    /// Message to sign (challenge token)
    pub message: String,
    /// Signature of the message
    pub signature: String,
    /// Recipient app name
    pub recipient: String,
}

/// NEAR wallet auth data type
pub struct NearWalletAuthDataType;

impl AuthDataType for NearWalletAuthDataType {
    fn method_name(&self) -> &str {
        "near_wallet"
    }

    fn parse_from_value(&self, value: Value) -> eyre::Result<Box<dyn std::any::Any + Send + Sync>> {
        // Try to deserialize as NearWalletAuthData
        match serde_json::from_value::<NearWalletAuthData>(value) {
            Ok(data) => Ok(Box::new(data)),
            Err(err) => Err(eyre::eyre!("Invalid NEAR wallet auth data: {}", err)),
        }
    }

    fn get_sample_structure(&self) -> Value {
        serde_json::json!({
            "account_id": "example.near",
            "public_key": "ed25519:...",
            "message": "challenge-token",
            "signature": "base64-encoded-signature",
            "recipient": "calimero"
        })
    }
}

/// NEAR wallet authentication provider
pub struct NearWalletProvider {
    storage: Arc<dyn Storage>,
    key_manager: KeyManager,
    token_manager: TokenManager,
    config: NearWalletConfig,
}

impl NearWalletProvider {
    /// Create a new NEAR wallet provider
    pub fn new(context: ProviderContext, config: NearWalletConfig) -> Self {
        Self {
            storage: context.storage,
            key_manager: context.key_manager,
            token_manager: context.token_manager,
            config,
        }
    }

    /// Verify a NEAR signature
    ///
    /// # Arguments
    ///
    /// * `public_key` - The public key to verify with
    /// * `message` - The challenge token that was signed
    /// * `signature` - The signature to verify
    /// * `nonce` - The nonce used in the signature (base64 encoded)
    /// * `recipient` - The recipient app name
    /// * `callback_url` - The callback URL
    ///
    /// # Returns
    ///
    /// * `eyre::Result<bool>` - Whether the signature is valid
    async fn verify_signature(
        &self,
        nonce: &str,
        message: &str,
        app: &str,
        signature_str: &str,
        public_key_str: &str,
    ) -> eyre::Result<bool> {
        // Parse the public key
        let public_key = PublicKey::from_str(public_key_str)
            .map_err(|e| eyre::eyre!("Invalid public key: {}", e))?;

        // Decode the base64 nonce
        let nonce_bytes = STANDARD
            .decode(nonce)
            .map_err(|e| eyre::eyre!("Invalid nonce base64: {}", e))?;

        let nonce_array: [u8; 32] = nonce_bytes
            .try_into()
            .map_err(|_| eyre::eyre!("Invalid nonce length"))?;

        // Create the payload that was signed
        let payload = create_payload(message, nonce_array, app);

        // Serialize the payload using borsh::to_vec
        let payload_bytes = borsh::to_vec(&payload)
            .map_err(|e| eyre::eyre!("Failed to serialize payload: {}", e))?;

        // Hash the payload - this is what was actually signed
        let hash = hash_bytes(&payload_bytes);

        // Decode the base64 signature
        let signature_bytes = STANDARD
            .decode(signature_str)
            .map_err(|e| eyre::eyre!("Invalid signature base64: {}", e))?;

        // Create signature from bytes
        let signature = Signature::from_parts(NearKeyType::ED25519, &signature_bytes)
            .map_err(|e| eyre::eyre!("Invalid signature: {}", e))?;

        // Verify the signature against the hashed payload
        let is_valid = signature.verify(&hash, &public_key);

        Ok(is_valid)
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
    /// * `eyre::Result<bool>` - Whether the public key belongs to the account
    async fn verify_account_owns_key(
        &self,
        account_id: &str,
        public_key: &str,
    ) -> eyre::Result<bool> {
        // Validate inputs
        if account_id.is_empty() {
            return Err(eyre::eyre!("Account ID cannot be empty"));
        }

        if public_key.is_empty() {
            return Err(eyre::eyre!("Public key cannot be empty"));
        }

        // Parse the account ID
        let account_id: AccountId = account_id
            .parse()
            .map_err(|err| eyre::eyre!("Invalid NEAR account ID: {}", err))?;

        // Parse the public key - use a variable first to avoid type annotation issues
        let public_key_result = public_key.parse::<near_crypto::PublicKey>();
        let parsed_public_key = public_key_result
            .map_err(|err| eyre::eyre!("Invalid NEAR public key format: {}", err))?;

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

    /// Get or create the root key for an account
    ///
    /// # Arguments
    ///
    /// * `account_id` - The account ID to get the root key for
    ///
    /// # Returns
    ///
    /// * `eyre::Result<Option<(String, Key)>>` - The root key ID and root key, if found
    async fn get_root_key_for_account(
        &self,
        account_id: &str,
    ) -> eyre::Result<Option<(String, Key)>> {
        // Create a hash of the account ID to use as a lookup key
        let mut hasher = Sha256::new();
        hasher.update(format!("near:{account_id}").as_bytes());
        let hash = hasher.finalize();
        let key_id = hex::encode(hash);

        // Look up the root key using KeyManager
        match self.key_manager.get_key(&key_id).await {
            Ok(Some(key)) => {
                // Check if the key is valid and is a root key
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

    /// Create a root key for an account
    ///
    /// # Arguments
    ///
    /// * `account_id` - The account ID to create a root key for
    /// * `public_key` - The public key to associate with the root key
    ///
    /// # Returns
    ///
    /// * `eyre::Result<(String, Key)>` - The created root key ID and root key
    async fn create_root_key(
        &self,
        account_id: &str,
        public_key: &str,
    ) -> eyre::Result<(String, Key)> {
        // Create a hash of the account ID to use as a key ID
        let mut hasher = Sha256::new();
        hasher.update(format!("near:{account_id}").as_bytes());
        let hash = hasher.finalize();
        let key_id = hex::encode(hash);

        // Create the root key
        let root_key = Key::new_root_key_with_permissions(
            public_key.to_string(),
            "near_wallet".to_string(),
            vec!["admin".to_string()], // Default admin permission
        );

        // Store the root key using KeyManager
        self.key_manager
            .set_key(&key_id, &root_key)
            .await
            .map_err(|err| eyre::eyre!("Failed to store root key: {}", err))?;

        Ok((key_id, root_key))
    }

    // TODO Decide what to do with this function
    /// Update the last used timestamp for a root key
    ///
    /// # Arguments
    ///
    /// * `key_id` - The root key ID to update
    ///
    /// # Returns
    ///
    /// * `eyre::Result<()>` - Success or error
    async fn update_last_used(&self, key_id: &str) -> eyre::Result<()> {
        // Get the current root key
        let key = self
            .key_manager
            .get_key(key_id)
            .await
            .map_err(|err| eyre::eyre!("Failed to get root key: {}", err))?
            .ok_or_else(|| eyre::eyre!("Root key not found"))?;

        // Update the last used timestamp
        // Note: We don't need to update last_used anymore since we removed that field

        // Save the updated key
        let _ = self
            .key_manager
            .set_key(key_id, &key)
            .await
            .map_err(|err| eyre::eyre!("Failed to update root key: {}", err));

        Ok(())
    }

    /// Core authentication logic for NEAR wallet
    ///
    /// This is the shared authentication logic used by both the request handler and verifier
    ///
    /// # Arguments
    ///
    /// * `account_id` - The NEAR account ID
    /// * `public_key` - The public key
    /// * `message` - The challenge token that was signed
    /// * `signature` - The signature
    /// * `recipient` - The recipient app name
    ///
    /// # Returns
    ///
    /// * `eyre::Result<(String, Vec<String>)>` - The key ID and permissions
    async fn authenticate_core(
        &self,
        account_id: &str,
        public_key: &str,
        message: String,
        signature: &str,
        recipient: &str,
    ) -> eyre::Result<(String, Vec<String>)> {
        // First verify that the message is a valid challenge token and get its claims
        let claims = self.token_manager.verify_challenge(&message).await?;

        // Then verify the signature using the nonce from the claims
        let signature_valid = self
            .verify_signature(&claims.nonce, &message, recipient, signature, public_key)
            .await?;

        if !signature_valid {
            return Err(eyre::eyre!(
                "Signature verification failed"
            ));
        }
        debug!("Signature verification successful");

        // Verify the account owns the key
        if !self.verify_account_owns_key(account_id, public_key).await? {
            return Err(eyre::eyre!(
                "Public key does not belong to account"
            ));
        }

        // Get or create the root key
        let (key_id, root_key) = match self.get_root_key_for_account(account_id).await? {
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

        let permissions = root_key.permissions.clone();
        debug!(
            "Returning permissions for key {}: {:?}",
            key_id, permissions
        );

        Ok((key_id, permissions))
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

/// NEAR wallet auth verifier
struct NearWalletVerifier {
    provider: Arc<NearWalletProvider>,
    auth_data: NearWalletAuthData,
}

#[async_trait]
impl AuthVerifierFn for NearWalletVerifier {
    async fn verify(&self) -> eyre::Result<AuthResponse> {
        let auth_data = &self.auth_data;

        // Authenticate using the core authentication logic
        let (key_id, permissions) = self
            .provider
            .authenticate_core(
                &auth_data.account_id,
                &auth_data.public_key,
                auth_data.message.clone(),
                &auth_data.signature,
                &auth_data.recipient,
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

// Implement Clone for NearWalletProvider
impl Clone for NearWalletProvider {
    fn clone(&self) -> Self {
        Self {
            storage: Arc::clone(&self.storage),
            key_manager: self.key_manager.clone(),
            token_manager: self.token_manager.clone(),
            config: self.config.clone(),
        }
    }
}

/// NEAR wallet specific request data
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct NearWalletRequest {
    /// Account ID of the NEAR wallet
    #[validate(length(min = 1, message = "Wallet address is required"))]
    pub wallet_address: String,

    /// Message that was signed (the challenge token)
    #[validate(length(min = 1, message = "Message is required"))]
    pub message: String,

    /// Signature of the message
    #[validate(length(min = 1, message = "Signature is required"))]
    pub signature: String,

    /// Recipient app name
    pub recipient: Option<String>,

    /// Callback URL
    pub callback_url: Option<String>,
}

#[async_trait]
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

    fn prepare_auth_data(&self, token_request: &TokenRequest) -> eyre::Result<Value> {
        // Parse the provider-specific data into our request type
        let near_data: NearWalletRequest =
            serde_json::from_value(token_request.provider_data.clone()).map_err(|e| {
                eyre::eyre!("Invalid NEAR wallet data: {}", e)
            })?;

        // Create NEAR-specific auth data JSON
        Ok(serde_json::json!({
            "account_id": near_data.wallet_address,
            "public_key": token_request.public_key,
            "message": near_data.message,
            "signature": near_data.signature,
            "recipient": near_data.recipient.unwrap_or_else(|| "calimero".to_string()),
            "callback_url": near_data.callback_url.unwrap_or_default()
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

        // Downcast to NearWalletAuthData
        let near_auth_data = auth_data
            .downcast_ref::<NearWalletAuthData>()
            .ok_or_else(|| eyre::eyre!("Failed to parse NEAR wallet auth data"))?;

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
            .to_string();

        // Create auth data
        let auth_data = NearWalletAuthData {
            account_id,
            public_key,
            message,
            signature,
            recipient: String::new(),
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
        Ok(serde_json::json!({
            "name": self.name(),
            "type": self.provider_type(),
            "configured": self.is_configured(),
            "rpc_url": self.config.rpc_url,
            "network": self.config.network,
        }))
    }

    async fn create_root_key(
        &self,
        public_key: &str,
        auth_method: &str,
        provider_data: Value,
    ) -> eyre::Result<bool> {
        let account_id = provider_data.get("account_id").unwrap().as_str().unwrap();
        // Create a hash of the account ID to use as a key ID
        let mut hasher = Sha256::new();
        hasher.update(format!("near:{account_id}").as_bytes());
        let hash = hasher.finalize();
        let key_id = hex::encode(hash);

        // Create the root key
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

        Ok(was_updated)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Creates a `Payload` struct from the provided message, nonce, recipient, and callback URL.
///
/// # Arguments
/// * `message` - The message to include in the payload.
/// * `nonce` - A 32-byte nonce.
/// * `recipient` - The recipient of the message.
///
/// # Returns
/// * `Payload` - The constructed payload.
fn create_payload(message: &str, nonce: [u8; 32], recipient: &str) -> Payload {
    Payload {
        tag: 2_147_484_061,
        message: message.to_owned(),
        nonce,
        recipient: recipient.to_owned(),
        callback_url: None,
    }
}

/// Hashes the given bytes using SHA-256.
///
/// # Arguments
/// * `bytes` - The bytes to hash.
///
/// # Returns
/// * `[u8; 32]` - The SHA-256 hash of the bytes.
fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    let mut hash_array = [0_u8; 32];
    hash_array.copy_from_slice(&result);
    hash_array
}

/// NEAR Wallet provider registration
pub struct NearWalletProviderRegistration;

impl ProviderRegistration for NearWalletProviderRegistration {
    fn provider_id(&self) -> &str {
        "near_wallet"
    }

    fn create_provider(
        &self,
        context: ProviderContext,
    ) -> Result<Box<dyn AuthProvider>, eyre::Error> {
        let config = context.config.near.clone();
        let provider = NearWalletProvider::new(context, config);
        Ok(Box::new(provider))
    }

    fn is_enabled(&self, config: &AuthConfig) -> bool {
        // Check if this provider is enabled in the config
        config
            .providers
            .get("near_wallet")
            .copied()
            .unwrap_or(false)
    }
}

// Register the NEAR wallet provider
register_auth_provider!(NearWalletProviderRegistration);

// Register the NEAR wallet auth data type
register_auth_data_type!(NearWalletAuthDataType);
