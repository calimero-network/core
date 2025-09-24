//! Relayer client for NEAR wallet verification
//!
//! This module provides a client for communicating with the Calimero relayer
//! to perform NEAR wallet verification operations without hitting rate limits.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, error, warn};
use url::Url;

/// Request structure for NEAR wallet verification via relayer
#[derive(Debug, Serialize)]
pub struct NearWalletVerificationRequest {
    /// The NEAR account ID to verify
    pub account_id: String,
    /// The public key to check ownership for
    pub public_key: String,
    /// Optional network override (defaults to configured network)
    pub network: Option<String>,
}

/// Response structure for NEAR wallet verification from relayer
#[derive(Debug, Deserialize)]
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

/// Error response for NEAR wallet verification from relayer
#[derive(Debug, Deserialize)]
pub struct NearWalletVerificationError {
    /// Error message
    pub error: String,
    /// Error code for categorization
    pub code: String,
}

/// Configuration for the relayer client
#[derive(Debug, Clone)]
pub struct RelayerClientConfig {
    /// Base URL of the relayer service
    pub relayer_url: Url,
    /// Request timeout duration
    pub timeout: Duration,
    /// Maximum number of retries
    pub max_retries: u32,
}

impl Default for RelayerClientConfig {
    fn default() -> Self {
        Self {
            relayer_url: Url::parse("http://localhost:63529").expect("valid URL"),
            timeout: Duration::from_secs(30),
            max_retries: 3,
        }
    }
}

/// Client for communicating with the Calimero relayer
#[derive(Debug, Clone)]
pub struct RelayerClient {
    client: Client,
    config: RelayerClientConfig,
}

impl RelayerClient {
    /// Create a new relayer client with default configuration
    pub fn new() -> Self {
        Self::with_config(RelayerClientConfig::default())
    }

    /// Create a new relayer client with custom configuration
    pub fn with_config(config: RelayerClientConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .user_agent("calimero-auth/0.1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self { client, config }
    }

    /// Create a new relayer client with custom URL
    pub fn with_url(relayer_url: Url) -> Self {
        let config = RelayerClientConfig {
            relayer_url,
            ..RelayerClientConfig::default()
        };
        Self::with_config(config)
    }

    /// Verify NEAR wallet ownership via the relayer
    pub async fn verify_near_wallet(
        &self,
        account_id: &str,
        public_key: &str,
        network: Option<&str>,
    ) -> eyre::Result<bool> {
        let request = NearWalletVerificationRequest {
            account_id: account_id.to_string(),
            public_key: public_key.to_string(),
            network: network.map(|s| s.to_string()),
        };

        debug!(
            "Sending NEAR wallet verification request to relayer: account_id={}, public_key={}",
            account_id, public_key
        );

        let url = self
            .config
            .relayer_url
            .join("/near/verify-wallet")
            .map_err(|e| eyre::eyre!("Failed to construct verification URL: {}", e))?;

        let mut attempt = 0;
        let mut last_error = None;

        while attempt < self.config.max_retries {
            attempt += 1;

            match self.client.post(url.clone()).json(&request).send().await {
                Ok(response) => {
                    let status = response.status();
                    
                    if status.is_success() {
                        match response.json::<NearWalletVerificationResponse>().await {
                            Ok(verification_response) => {
                                debug!(
                                    "NEAR wallet verification successful: owns_key={}",
                                    verification_response.owns_key
                                );
                                return Ok(verification_response.owns_key);
                            }
                            Err(err) => {
                                error!("Failed to parse verification response: {}", err);
                                last_error = Some(format!("Parse error: {}", err));
                            }
                        }
                    } else {
                        // Try to parse error response
                        match response.json::<NearWalletVerificationError>().await {
                            Ok(error_response) => {
                                error!(
                                    "Relayer returned error: {} (code: {})",
                                    error_response.error, error_response.code
                                );
                                last_error = Some(error_response.error);
                            }
                            Err(_) => {
                                let error_msg = format!("HTTP {}: {}", status, status.canonical_reason().unwrap_or("Unknown"));
                                error!("Relayer HTTP error: {}", error_msg);
                                last_error = Some(error_msg);
                            }
                        }
                    }
                }
                Err(err) => {
                    warn!(
                        "Failed to send verification request to relayer (attempt {}/{}): {}",
                        attempt, self.config.max_retries, err
                    );
                    last_error = Some(err.to_string());
                }
            }

            // Wait before retrying (unless this was the last attempt)
            if attempt < self.config.max_retries {
                let delay_ms = 100 * (2_u64.pow(attempt as u32));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        // All retries failed
        let error_msg = last_error.unwrap_or_else(|| "Unknown error".to_string());
        Err(eyre::eyre!(
            "Failed to verify NEAR wallet via relayer after {} attempts: {}",
            self.config.max_retries,
            error_msg
        ))
    }

    /// Check if the relayer is available
    pub async fn health_check(&self) -> bool {
        let url = match self.config.relayer_url.join("/health") {
            Ok(url) => url,
            Err(_) => return false,
        };

        match self.client.get(url).send().await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    /// Get the configured relayer URL
    pub fn relayer_url(&self) -> &Url {
        &self.config.relayer_url
    }
}

impl Default for RelayerClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relayer_client_creation() {
        let client = RelayerClient::new();
        assert_eq!(
            client.relayer_url().as_str(),
            "http://localhost:63529/"
        );
    }

    #[test]
    fn test_relayer_client_with_custom_url() {
        let url = Url::parse("http://example.com:8080").unwrap();
        let client = RelayerClient::with_url(url.clone());
        assert_eq!(client.relayer_url(), &url);
    }

    #[test]
    fn test_relayer_client_config() {
        let config = RelayerClientConfig {
            relayer_url: Url::parse("http://test.example.com").unwrap(),
            timeout: Duration::from_secs(60),
            max_retries: 5,
        };

        let client = RelayerClient::with_config(config.clone());
        assert_eq!(client.config.relayer_url, config.relayer_url);
        assert_eq!(client.config.timeout, config.timeout);
        assert_eq!(client.config.max_retries, config.max_retries);
    }
}

