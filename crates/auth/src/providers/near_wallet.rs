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
use sha2::{Digest, Sha256};
use tracing::{debug, error};

use crate::config::NearWalletConfig;
use crate::providers::jwt::TokenManager;
use crate::storage::{deserialize, prefixes, serialize, RootKey, Storage, StorageError};
use crate::{
    AuthError, AuthProvider, AuthRequestVerifier, AuthResponse, AuthVerifierFn, RequestValidator,
};

/// NEAR wallet authentication provider
pub struct NearWalletProvider {
    config: NearWalletConfig,
    storage: Arc<dyn Storage>,
    token_manager: Option<TokenManager>,
}

impl NearWalletProvider {
    /// Create a new NEAR wallet provider
    ///
    /// # Arguments
    ///
    /// * `config` - NEAR wallet configuration
    /// * `storage` - Storage backend
    pub fn new(config: NearWalletConfig, storage: Arc<dyn Storage>) -> Self {
        Self {
            config,
            storage,
            token_manager: None,
        }
    }

    /// Create a new NEAR wallet provider with a token generator
    ///
    /// # Arguments
    ///
    /// * `config` - NEAR wallet configuration
    /// * `storage` - Storage backend
    /// * `token_manager` - JWT token manager
    pub fn with_token_manager(
        config: NearWalletConfig,
        storage: Arc<dyn Storage>,
        token_manager: TokenManager,
    ) -> Self {
        Self {
            config,
            storage,
            token_manager: Some(token_manager),
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
        // Parse the public key
        let public_key = PublicKey::from_str(public_key_str).map_err(|_| {
            AuthError::AuthenticationFailed("Invalid NEAR public key format".to_string())
        })?;

        // Parse the signature
        let signature_bytes = STANDARD.decode(signature_str).map_err(|_| {
            AuthError::AuthenticationFailed("Invalid NEAR signature encoding".to_string())
        })?;

        let signature =
            Signature::from_parts(KeyType::ED25519, &signature_bytes).map_err(|_| {
                AuthError::AuthenticationFailed("Invalid NEAR signature format".to_string())
            })?;

        // Verify the signature
        if !signature.verify(message, &public_key) {
            return Err(AuthError::AuthenticationFailed(
                "Invalid NEAR signature".to_string(),
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
        // Connect to the NEAR RPC
        let client = JsonRpcClient::connect(&self.config.rpc_url);

        // Parse the account ID
        let account_id: AccountId = account_id
            .parse()
            .map_err(|_| AuthError::AuthenticationFailed("Invalid NEAR account ID".to_string()))?;

        // Query the account's access keys
        let request = methods::query::RpcQueryRequest {
            block_reference: BlockReference::Finality(Finality::Final),
            request: QueryRequest::ViewAccessKey {
                account_id: account_id.clone(),
                public_key: public_key.parse().map_err(|_| {
                    AuthError::AuthenticationFailed("Invalid NEAR public key format".to_string())
                })?,
            },
        };

        // Send the request
        let response = client.call(request).await;

        match response {
            Ok(_) => Ok(true), // If we get a valid response, the key belongs to the account
            Err(err) => {
                debug!("Failed to verify NEAR account key: {}", err);
                Ok(false) // Key doesn't belong to the account
            }
        }
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

    /// Handle authentication and generate tokens
    ///
    /// # Arguments
    ///
    /// * `request` - The request to authenticate
    /// * `account_id` - The account ID
    /// * `public_key` - The public key
    /// * `signature` - The signature
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
        // Verify the signature
        let signature = request
            .headers()
            .get("x-near-signature")
            .ok_or_else(|| AuthError::AuthenticationFailed("Missing NEAR signature".to_string()))?
            .to_str()
            .map_err(|_| AuthError::AuthenticationFailed("Invalid NEAR signature".to_string()))?;

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

        // If we have a token manager, generate a JWT token for the client
        if let Some(token_manager) = &self.token_manager {
            // Generate a client ID and client key, or find an existing one
            let client_id = format!("client_{}", Utc::now().timestamp());

            // Generate tokens for the client
            match token_manager
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
                    // Continue anyway, authentication is still valid
                }
            }
        }

        Ok(AuthResponse {
            is_valid: true,
            key_id: Some(key_id),
            permissions,
        })
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

/// Extracted auth data for NEAR wallet
#[derive(Debug, Serialize, Deserialize)]
struct NearAuthData {
    account_id: String,
    public_key: String,
    message: Vec<u8>,
    signature: String,
}

/// NEAR wallet auth verifier
struct NearWalletVerifier {
    provider: Arc<NearWalletProvider>,
    auth_data: NearAuthData,
}

#[async_trait]
impl AuthVerifierFn for NearWalletVerifier {
    async fn verify(&self) -> Result<AuthResponse, AuthError> {
        let auth_data = &self.auth_data;

        // Verify the signature
        self.provider
            .verify_signature(
                &auth_data.public_key,
                &auth_data.message,
                &auth_data.signature,
            )
            .await?;

        // Verify the account owns the key
        if !self
            .provider
            .verify_account_owns_key(&auth_data.account_id, &auth_data.public_key)
            .await?
        {
            return Err(AuthError::AuthenticationFailed(
                "Public key does not belong to account".to_string(),
            ));
        }

        // Get or create the root key
        let (key_id, _root_key) = match self
            .provider
            .get_root_key_for_account(&auth_data.account_id)
            .await?
        {
            Some((key_id, root_key)) => {
                // Update the last used timestamp
                self.provider.update_last_used(&key_id).await?;
                (key_id, root_key)
            }
            None => {
                // Create a new root key
                self.provider
                    .create_root_key(&auth_data.account_id, &auth_data.public_key)
                    .await?
            }
        };

        // For now, grant admin permissions to all NEAR wallets
        // In a real implementation, you would look up the permissions from storage
        let permissions = vec!["admin".to_string()];

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
        let auth_data = NearAuthData {
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
}
