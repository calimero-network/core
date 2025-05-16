use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

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
    /// Secret key for signing and verifying tokens
    pub secret: String,
    
    /// Token issuer
    pub issuer: String,
    
    /// Access token expiry time in seconds (default: 1 hour)
    pub access_token_expiry: u64,
    
    /// Refresh token expiry time in seconds (default: 30 days)
    pub refresh_token_expiry: u64,
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
