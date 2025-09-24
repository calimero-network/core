//! NEAR wallet verification module for the relayer service
//!
//! This module provides NEAR wallet verification functionality that can be used
//! by external services like the auth service to verify wallet ownership without
//! hitting rate limits.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use eyre::Result as EyreResult;
use near_crypto::PublicKey;
use near_jsonrpc_client::{auth, methods, JsonRpcClient};
use near_primitives::types::{AccountId, BlockReference, Finality};
use near_primitives::views::QueryRequest;
use serde::{Deserialize, Serialize};
use std::env;
use std::str::FromStr;
use tracing::{debug, error};

use crate::config::{ProtocolConfig, RelayerConfig};

/// Request structure for NEAR wallet verification
#[derive(Debug, Deserialize, Serialize)]
pub struct NearWalletVerificationRequest {
    /// The NEAR account ID to verify
    pub account_id: String,
    /// The public key to check ownership for
    pub public_key: String,
    /// Optional network override (defaults to configured network)
    pub network: Option<String>,
}

/// Response structure for NEAR wallet verification
#[derive(Debug, Deserialize, Serialize)]
pub struct NearWalletVerificationResponse {
    /// Whether the account owns the public key
    pub owns_key: bool,
    /// The account ID that was verified
    pub account_id: String,
    /// The public key that was checked
    pub public_key: String,
    /// The network that was used for verification
    pub network: String,
}

/// Error response for NEAR wallet verification
#[derive(Debug, Deserialize, Serialize)]
pub struct NearWalletVerificationError {
    /// Error message
    pub error: String,
    /// Error code for categorization
    pub code: String,
}

/// NEAR wallet verification service
pub struct NearWalletVerifier {
    config: RelayerConfig,
}

impl NearWalletVerifier {
    /// Create a new NEAR wallet verifier
    pub fn new(config: RelayerConfig) -> Self {
        Self { config }
    }

    /// Get the NEAR protocol configuration
    fn get_near_config(&self) -> Option<&ProtocolConfig> {
        self.config.protocols.get("near").filter(|c| c.enabled)
    }

    /// Create a NEAR JSON RPC client for the specified network
    fn create_rpc_client(&self, network: Option<&str>) -> EyreResult<(JsonRpcClient, String)> {
        let near_config = self.get_near_config()
            .ok_or_else(|| eyre::eyre!("NEAR protocol not enabled in relayer configuration"))?;

        // Use provided network or default from config
        let network_name = network.unwrap_or(&near_config.network);
        
        // Create RPC client with the configured URL
        let mut client = JsonRpcClient::connect(near_config.rpc_url.clone());

        // Apply NEAR API key authentication if available
        if let Ok(api_key) = env::var("NEAR_API_KEY") {
            client = client.header(auth::ApiKey::new(&api_key).map_err(|e| eyre::eyre!("Invalid API key: {e}"))?);
            client = client.header(auth::Authorization::bearer(&api_key).map_err(|e| eyre::eyre!("Invalid API key: {e}"))?);
        }

        Ok((client, network_name.to_string()))
    }

    /// Verify if a NEAR account owns a specific public key
    pub async fn verify_account_owns_key(
        &self,
        account_id: &str,
        public_key: &str,
        network: Option<&str>,
    ) -> EyreResult<bool> {
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

        // Parse the public key
        let parsed_public_key = PublicKey::from_str(public_key)
            .map_err(|err| eyre::eyre!("Invalid NEAR public key format: {}", err))?;

        // Create RPC client
        let (client, _network_name) = self.create_rpc_client(network)?;

        // Query the account's access keys with retry logic
        let max_retries = 3;
        let mut attempt = 0;
        let mut last_error = None;

        while attempt < max_retries {
            attempt += 1;

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
                Ok(_) => {
                    debug!(
                        "Successfully verified that account {} owns public key {}",
                        account_id, public_key
                    );
                    return Ok(true);
                }
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
        if let Some(error) = last_error {
            debug!(
                "Final error verifying account key ownership: {}",
                error
            );
        }

        // If we get an error, it might be because the key doesn't exist
        // But it could also be a network issue, so we'll return false
        Ok(false)
    }
}

/// Handler for NEAR wallet verification requests
pub async fn near_wallet_verification_handler(
    State(config): State<RelayerConfig>,
    Json(request): Json<NearWalletVerificationRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    debug!("Received NEAR wallet verification request: {:?}", request);

    // Validate request
    if request.account_id.is_empty() {
        let error = NearWalletVerificationError {
            error: "Account ID cannot be empty".to_string(),
            code: "INVALID_ACCOUNT_ID".to_string(),
        };
        return Ok((StatusCode::BAD_REQUEST, Json(error)).into_response());
    }

    if request.public_key.is_empty() {
        let error = NearWalletVerificationError {
            error: "Public key cannot be empty".to_string(),
            code: "INVALID_PUBLIC_KEY".to_string(),
        };
        return Ok((StatusCode::BAD_REQUEST, Json(error)).into_response());
    }

    // Create verifier and perform verification
    let verifier = NearWalletVerifier::new(config);

    match verifier
        .verify_account_owns_key(
            &request.account_id,
            &request.public_key,
            request.network.as_deref(),
        )
        .await
    {
        Ok(owns_key) => {
            let network = request.network.unwrap_or_else(|| {
                verifier
                    .get_near_config()
                    .map(|c| c.network.clone())
                    .unwrap_or_else(|| "testnet".to_string())
            });

            let response = NearWalletVerificationResponse {
                owns_key,
                account_id: request.account_id,
                public_key: request.public_key,
                network,
            };

            debug!("NEAR wallet verification result: {:?}", response);
            Ok(Json(response).into_response())
        }
        Err(err) => {
            error!("NEAR wallet verification failed: {}", err);

            let error_code = if err.to_string().contains("Invalid") {
                "INVALID_INPUT"
            } else if err.to_string().contains("not enabled") {
                "SERVICE_UNAVAILABLE"
            } else {
                "VERIFICATION_FAILED"
            };

            let error = NearWalletVerificationError {
                error: err.to_string(),
                code: error_code.to_string(),
            };

            let status = match error_code {
                "INVALID_INPUT" => StatusCode::BAD_REQUEST,
                "SERVICE_UNAVAILABLE" => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };

            Ok((status, Json(error)).into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProtocolConfig, RelayerConfig};
    use std::collections::BTreeMap;
    use std::net::SocketAddr;
    use url::Url;

    fn create_test_config() -> RelayerConfig {
        let mut protocols = BTreeMap::new();
        protocols.insert(
            "near".to_string(),
            ProtocolConfig {
                enabled: true,
                network: "testnet".to_string(),
                rpc_url: Url::parse("https://rpc.testnet.near.org").unwrap(),
                contract_id: "test.testnet".to_string(),
                credentials: None,
            },
        );

        RelayerConfig {
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            protocols,
        }
    }

    #[test]
    fn test_near_wallet_verifier_creation() {
        let config = create_test_config();
        let verifier = NearWalletVerifier::new(config);

        assert!(verifier.get_near_config().is_some());
    }

    #[test]
    fn test_invalid_account_id() {
        let config = create_test_config();
        let verifier = NearWalletVerifier::new(config);

        // Test with empty account ID
        let result = tokio_test::block_on(async {
            verifier
                .verify_account_owns_key("", "ed25519:test", None)
                .await
        });

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_invalid_public_key() {
        let config = create_test_config();
        let verifier = NearWalletVerifier::new(config);

        // Test with empty public key
        let result = tokio_test::block_on(async {
            verifier
                .verify_account_owns_key("test.testnet", "", None)
                .await
        });

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }
}
