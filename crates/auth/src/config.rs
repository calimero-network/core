use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};

/// Authentication service configuration
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    /// Listen address
    #[serde(default = "default_listen_addr")]
    pub listen_addr: SocketAddr,

    /// Node URL
    pub node_url: String,

    /// JWT configuration
    pub jwt: JwtConfig,

    /// Storage configuration
    pub storage: StorageConfig,

    /// CORS configuration
    #[serde(default)]
    pub cors: CorsConfig,

    /// Security configuration
    #[serde(default)]
    pub security: SecurityConfig,

    /// Authentication providers configuration
    #[serde(default)]
    pub providers: HashMap<String, bool>,

    /// NEAR wallet configuration
    #[serde(default)]
    pub near: NearWalletConfig,
}

fn default_listen_addr() -> SocketAddr {
    "127.0.0.1:3001".parse().unwrap()
}

/// JWT configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtConfig {
    /// Token issuer
    pub issuer: String,

    /// Access token expiry time in seconds (default: 1 hour)
    #[serde(default = "default_access_token_expiry")]
    pub access_token_expiry: u64,

    /// Refresh token expiry time in seconds (default: 30 days)
    #[serde(default = "default_refresh_token_expiry")]
    pub refresh_token_expiry: u64,
}

fn default_access_token_expiry() -> u64 {
    3600 // 1 hour
}

fn default_refresh_token_expiry() -> u64 {
    30 * 24 * 3600 // 30 days
}

/// Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StorageConfig {
    /// RocksDB storage
    #[serde(rename = "rocksdb")]
    RocksDB {
        /// The path to the RocksDB database
        path: PathBuf,
    },

    /// In-memory storage (for development and testing)
    #[serde(rename = "memory")]
    Memory,
}

/// NEAR wallet configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NearWalletConfig {
    /// Network
    pub network: String,

    /// RPC URL for NEAR network
    pub rpc_url: String,

    /// Wallet URL
    pub wallet_url: String,

    /// Helper URL (optional)
    pub helper_url: Option<String>,
}

impl Default for NearWalletConfig {
    fn default() -> Self {
        Self {
            network: "testnet".to_string(),
            rpc_url: "https://rpc.testnet.near.org".to_string(),
            wallet_url: "https://wallet.testnet.near.org".to_string(),
            helper_url: None,
        }
    }
}

/// CORS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsConfig {
    /// Allow all origins
    #[serde(default)]
    pub allow_all_origins: bool,

    /// Allowed origins (if allow_all_origins is false)
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// Allowed methods
    #[serde(default)]
    pub allowed_methods: Vec<String>,

    /// Allowed headers
    #[serde(default)]
    pub allowed_headers: Vec<String>,

    /// Expose headers
    #[serde(default)]
    pub exposed_headers: Vec<String>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allow_all_origins: false,
            allowed_origins: Vec::new(),
            allowed_methods: vec![
                "GET".to_string(),
                "POST".to_string(),
                "PUT".to_string(),
                "DELETE".to_string(),
                "OPTIONS".to_string(),
            ],
            allowed_headers: vec![
                "Authorization".to_string(),
                "Content-Type".to_string(),
                "Accept".to_string(),
            ],
            exposed_headers: Vec::new(),
        }
    }
}

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// CSRF secret key (must be at least 32 bytes when decoded from base64)
    #[serde(default = "default_csrf_secret")]
    pub csrf_secret: String,

    /// Rate limit configuration (requests per minute)
    #[serde(default = "default_rate_limit")]
    pub rate_limit: u32,

    /// Maximum request body size in bytes
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            csrf_secret: default_csrf_secret(),
            rate_limit: default_rate_limit(),
            max_body_size: default_max_body_size(),
        }
    }
}

fn default_csrf_secret() -> String {
    // Generate a random 32-byte key and encode as base64
    let key = rand::random::<[u8; 32]>();
    STANDARD.encode(key)
}

fn default_rate_limit() -> u32 {
    50 // 50 requests per minute
}

fn default_max_body_size() -> usize {
    1024 * 1024 // 1MB
}

/// Load the configuration from a file
///
/// # Arguments
///
/// * `path` - The path to the configuration file
///
/// # Returns
///
/// * `Result<AuthConfig, eyre::Error>` - The loaded configuration
pub fn load_config(path: &str) -> eyre::Result<AuthConfig> {
    let config = config::Config::builder()
        .add_source(config::File::with_name(path))
        .add_source(config::Environment::with_prefix("AUTH").separator("__"))
        .build()?
        .try_deserialize()?;

    Ok(config)
}
