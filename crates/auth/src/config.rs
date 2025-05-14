use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Authentication service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// The address to listen on
    #[serde(default = "default_listen_addr")]
    pub listen_addr: SocketAddr,

    /// The URL of the node to forward authenticated requests to
    pub node_url: String,

    /// JWT settings
    #[serde(default)]
    pub jwt: JwtConfig,

    /// Storage settings
    pub storage: StorageConfig,

    /// Enabled authentication providers
    #[serde(default)]
    pub providers: ProvidersConfig,

    /// CORS settings
    #[serde(default)]
    pub cors: CorsConfig,

    /// NEAR wallet configuration
    pub near: NearWalletConfig,
}

fn default_listen_addr() -> SocketAddr {
    "127.0.0.1:3001".parse().unwrap()
}

/// JWT configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtConfig {
    /// Secret key for signing and verifying tokens
    #[serde(default = "default_jwt_secret")]
    pub secret: String,
    /// Token issuer
    #[serde(default = "default_jwt_issuer")]
    pub issuer: String,
    /// Access token expiry time in seconds (default: 1 hour)
    #[serde(default = "default_access_token_expiry")]
    pub access_token_expiry: u64,
    /// Refresh token expiry time in seconds (default: 30 days)
    #[serde(default = "default_refresh_token_expiry")]
    pub refresh_token_expiry: u64,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            secret: default_jwt_secret(),
            issuer: default_jwt_issuer(),
            access_token_expiry: default_access_token_expiry(),
            refresh_token_expiry: default_refresh_token_expiry(),
        }
    }
}

// Define default functions for JwtConfig fields
fn default_jwt_secret() -> String {
    "insecure_default_secret_please_change_in_production".to_string()
}

fn default_jwt_issuer() -> String {
    "calimero-auth".to_string()
}

fn default_access_token_expiry() -> u64 {
    3600 // 1 hour
}

fn default_refresh_token_expiry() -> u64 {
    2592000 // 30 days
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

    /// Redis storage
    #[serde(rename = "redis")]
    Redis {
        /// The Redis URL
        url: String,

        /// Connection pool size
        #[serde(default = "default_connection_pool_size")]
        pool_size: usize,
    },

    /// PostgreSQL storage
    #[serde(rename = "postgres")]
    Postgres {
        /// The PostgreSQL connection URL
        url: String,

        /// Connection pool size
        #[serde(default = "default_connection_pool_size")]
        pool_size: usize,
    },

    /// SQLite storage
    #[serde(rename = "sqlite")]
    SQLite {
        /// The path to the SQLite database
        path: PathBuf,
    },

    /// In-memory storage (for development only)
    #[serde(rename = "memory")]
    Memory,
}

fn default_connection_pool_size() -> usize {
    10
}

/// Authentication providers configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvidersConfig {
    /// Enable NEAR wallet authentication
    #[serde(default = "default_false")]
    pub near_wallet: bool,

    /// Enable Ethereum wallet authentication
    #[serde(default)]
    pub eth_wallet: bool,

    /// Configure Ethereum wallet settings
    #[serde(default)]
    pub eth_wallet_config: EthWalletConfig,

    /// Enable Starknet wallet authentication
    #[serde(default)]
    pub starknet_wallet: bool,

    /// Configure Starknet wallet settings
    #[serde(default)]
    pub starknet_wallet_config: StarknetWalletConfig,

    /// Enable Internet Computer authentication
    #[serde(default)]
    pub icp: bool,

    /// Configure Internet Computer settings
    #[serde(default)]
    pub icp_config: IcpConfig,

    /// Enable OAuth authentication
    #[serde(default)]
    pub oauth: bool,

    /// Configure OAuth providers
    #[serde(default)]
    pub oauth_providers: HashMap<String, OAuthProviderConfig>,
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

/// NEAR wallet configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NearWalletConfig {
    /// Network
    pub network: String,

    /// RPC URL for NEAR network
    #[serde(default = "default_near_mainnet_url")]
    pub rpc_url: String,

    /// Wallet URL
    pub wallet_url: String,

    /// Helper URL (optional)
    pub helper_url: Option<String>,
}

fn default_near_mainnet_url() -> String {
    "https://rpc.mainnet.near.org".to_string()
}

/// Ethereum wallet configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EthWalletConfig {
    /// RPC URL for Ethereum network
    #[serde(default = "default_eth_mainnet_url")]
    pub rpc_url: String,

    /// Chain ID
    #[serde(default = "default_eth_chain_id")]
    pub chain_id: u64,
}

fn default_eth_mainnet_url() -> String {
    "https://eth.llamarpc.com".to_string()
}

fn default_eth_chain_id() -> u64 {
    1 // Ethereum mainnet
}

/// Starknet wallet configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StarknetWalletConfig {
    /// RPC URL for Starknet network
    #[serde(default = "default_starknet_mainnet_url")]
    pub rpc_url: String,

    /// Chain ID
    #[serde(default = "default_starknet_chain_id")]
    pub chain_id: String,
}

fn default_starknet_mainnet_url() -> String {
    "https://starknet-mainnet.infura.io/v3/".to_string()
}

fn default_starknet_chain_id() -> String {
    "SN_MAIN".to_string()
}

/// Internet Computer configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IcpConfig {
    /// Host URL for Internet Computer
    #[serde(default = "default_icp_host")]
    pub host: String,
}

fn default_icp_host() -> String {
    "https://ic0.app".to_string()
}

/// OAuth provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthProviderConfig {
    /// Client ID
    pub client_id: String,

    /// Client secret
    pub client_secret: String,

    /// Authorization URL
    pub auth_url: String,

    /// Token URL
    pub token_url: String,

    /// User info URL (if applicable)
    pub user_info_url: Option<String>,

    /// Redirect URL
    pub redirect_url: String,

    /// Scopes to request
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// CORS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsConfig {
    /// Allow all origins
    #[serde(default = "default_true")]
    pub allow_all_origins: bool,

    /// Allowed origins (if allow_all_origins is false)
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// Allow credentials
    #[serde(default = "default_true")]
    pub allow_credentials: bool,

    /// Allowed methods
    #[serde(default = "default_allowed_methods")]
    pub allowed_methods: Vec<String>,

    /// Allowed headers
    #[serde(default = "default_allowed_headers")]
    pub allowed_headers: Vec<String>,

    /// Expose headers
    #[serde(default)]
    pub exposed_headers: Vec<String>,

    /// Max age in seconds
    #[serde(default = "default_max_age")]
    pub max_age: u64,
}

fn default_allowed_methods() -> Vec<String> {
    vec![
        "GET".to_string(),
        "POST".to_string(),
        "PUT".to_string(),
        "DELETE".to_string(),
        "OPTIONS".to_string(),
    ]
}

fn default_allowed_headers() -> Vec<String> {
    vec![
        "Authorization".to_string(),
        "Content-Type".to_string(),
        "Accept".to_string(),
    ]
}

fn default_max_age() -> u64 {
    86400 // 24 hours
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allow_all_origins: true,
            allowed_origins: Vec::new(),
            allow_credentials: true,
            allowed_methods: default_allowed_methods(),
            allowed_headers: default_allowed_headers(),
            exposed_headers: Vec::new(),
            max_age: default_max_age(),
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

/// Generate a secure random secret for JWT tokens
fn generate_default_secret() -> String {
    use rand::{thread_rng, Rng};
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    const SECRET_LEN: usize = 32;

    let mut rng = thread_rng();
    let secret: String = (0..SECRET_LEN)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect();

    secret
}

/// Generate a default configuration
///
/// # Returns
///
/// * `AuthConfig` - The default configuration
pub fn default_config() -> AuthConfig {
    let default_db_path = PathBuf::from("./data/auth_db");

    AuthConfig {
        listen_addr: default_listen_addr(),
        node_url: "http://localhost:2428".to_string(),
        jwt: JwtConfig {
            secret: generate_default_secret(),
            access_token_expiry: default_access_token_expiry(),
            refresh_token_expiry: default_refresh_token_expiry(),
            issuer: default_jwt_issuer(),
        },
        storage: StorageConfig::RocksDB {
            path: default_db_path,
        },
        providers: ProvidersConfig::default(),
        cors: CorsConfig::default(),
        near: NearWalletConfig {
            network: "mainnet".to_string(),
            rpc_url: "https://rpc.mainnet.near.org".to_string(),
            wallet_url: "https://wallet.mainnet.near.org".to_string(),
            helper_url: None,
        },
    }
}
