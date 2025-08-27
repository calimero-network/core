//! Abstract traits for Calimero client functionality
//!
//! This module defines the core traits that different client implementations
//! must implement. This allows for maximum flexibility while maintaining
//! a consistent interface.

use async_trait::async_trait;
use eyre::Result;
use url::Url;

use crate::storage::JwtToken;

/// Abstract trait for client token storage operations
///
/// This trait defines the interface for storing and retrieving JWT tokens
/// for different nodes. Implementations can use file systems, databases,
/// secure storage, or any other backend.
#[async_trait]
pub trait ClientStorage: Send + Sync {
    /// Load tokens for a specific node
    ///
    /// Returns `Ok(Some(tokens))` if tokens exist, `Ok(None)` if no tokens
    /// are stored for the node, or an error if the operation fails.
    async fn load_tokens(&self, node_name: &str) -> Result<Option<JwtToken>>;

    /// Save tokens for a specific node
    ///
    /// Stores the provided tokens for the specified node. This should
    /// overwrite any existing tokens for the node.
    async fn save_tokens(&self, node_name: &str, tokens: &JwtToken) -> Result<()>;

    /// Update tokens for a specific node (load, modify, save)
    ///
    /// This is a convenience method that loads existing tokens, applies
    /// modifications, and saves the result. Implementations can optimize
    /// this operation if needed.
    async fn update_tokens(&self, node_name: &str, new_tokens: &JwtToken) -> Result<()> {
        self.save_tokens(node_name, new_tokens).await
    }

    /// Remove tokens for a specific node
    ///
    /// Removes any stored tokens for the specified node. This is useful
    /// for logout operations or clearing invalid tokens.
    async fn remove_tokens(&self, node_name: &str) -> Result<()> {
        // Default implementation: just save None
        self.save_tokens(node_name, &JwtToken::default()).await
    }

    /// List all nodes that have stored tokens
    ///
    /// Returns a list of node names that currently have tokens stored.
    /// This is useful for management operations.
    async fn list_nodes(&self) -> Result<Vec<String>> {
        // Default implementation: return empty list
        Ok(Vec::new())
    }
}

/// Abstract trait for client authentication operations
///
/// This trait defines the interface for authenticating with Calimero APIs.
/// Different implementations can support various authentication methods:
/// - Browser-based OAuth flows
/// - API key authentication
/// - Username/password
/// - Hardware security modules
#[async_trait]
pub trait ClientAuthenticator: Send + Sync {
    /// Authenticate with the API and return tokens
    ///
    /// This is the main authentication method. It should handle the entire
    /// authentication flow and return valid JWT tokens.
    async fn authenticate(&self, api_url: &Url) -> Result<JwtToken>;

    /// Refresh authentication tokens
    ///
    /// Attempts to refresh expired tokens using a refresh token. This
    /// should be called when the main tokens expire.
    async fn refresh_tokens(&self, refresh_token: &str) -> Result<JwtToken>;

    /// Handle authentication failure (e.g., open browser, show instructions)
    ///
    /// This method is called when authentication fails and the user needs
    /// to take action. Implementations might:
    /// - Open a browser for OAuth
    /// - Display instructions to the user
    /// - Retry with different credentials
    async fn handle_auth_failure(&self, api_url: &Url) -> Result<JwtToken>;

    /// Check if authentication is required for a given API URL
    ///
    /// Some APIs might not require authentication. This method checks
    /// whether the given URL requires authentication.
    async fn check_auth_required(&self, api_url: &Url) -> Result<bool>;

    /// Get authentication method description
    ///
    /// Returns a human-readable description of how this authenticator
    /// works, useful for user instructions.
    fn get_auth_method(&self) -> &'static str;

    /// Check if the authenticator supports refresh
    ///
    /// Returns true if this authenticator supports token refresh,
    /// false otherwise.
    fn supports_refresh(&self) -> bool {
        true // Default to true, implementations can override
    }
}

/// Abstract trait for client configuration management
///
/// This trait defines the interface for managing client configuration,
/// including node connections, settings, and preferences.
#[async_trait]
pub trait ClientConfig<A, S>: Send + Sync
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    /// Get connection information for a specific node
    ///
    /// Returns the connection configuration for the specified node,
    /// including URL, authentication settings, and other parameters.
    async fn get_connection(
        &self,
        node_name: &str,
    ) -> Result<Option<crate::connection::ConnectionInfo<A, S>>>;

    /// Get the active node name
    ///
    /// Returns the name of the currently active/default node.
    fn get_active_node(&self) -> Option<&str>;

    /// Set the active node
    ///
    /// Changes the active node to the specified name.
    async fn set_active_node(&self, node_name: &str) -> Result<()>;

    /// List all configured nodes
    ///
    /// Returns a list of all node names that are configured.
    async fn list_nodes(&self) -> Result<Vec<String>>;

    /// Add a new node configuration
    ///
    /// Adds configuration for a new node with the given parameters.
    async fn add_node(&self, name: &str, url: &Url, auth_config: Option<&str>) -> Result<()>;

    /// Remove a node configuration
    ///
    /// Removes the configuration for the specified node.
    async fn remove_node(&self, name: &str) -> Result<()>;

    /// Get client settings
    ///
    /// Returns general client settings like timeouts, retry policies, etc.
    fn get_settings(&self) -> Result<ClientSettings>;

    /// Update client settings
    ///
    /// Updates the client settings with new values.
    async fn update_settings(&self, settings: &ClientSettings) -> Result<()>;
}

/// Client settings configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClientSettings {
    /// HTTP request timeout in seconds
    pub request_timeout: u64,
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Retry delay in milliseconds
    pub retry_delay_ms: u64,
    /// Whether to use HTTP/2
    pub use_http2: bool,
    /// User agent string
    pub user_agent: String,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            request_timeout: 30,
            max_retries: 3,
            retry_delay_ms: 1000,
            use_http2: true,
            user_agent: format!("client/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// HTTP client configuration
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// Request timeout
    pub timeout: std::time::Duration,
    /// Maximum number of retries
    pub max_retries: u32,
    /// Retry delay
    pub retry_delay: std::time::Duration,
    /// Custom headers
    pub headers: std::collections::HashMap<String, String>,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout: std::time::Duration::from_secs(30),
            max_retries: 3,
            retry_delay: std::time::Duration::from_millis(1000),
            headers: std::collections::HashMap::new(),
        }
    }
}
