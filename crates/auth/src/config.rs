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

    /// Username/password configuration
    #[serde(default)]
    pub user_password: UserPasswordConfig,

    /// Development/testing configuration
    #[serde(default)]
    pub development: DevelopmentConfig,
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

/// Username/password configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPasswordConfig {
    /// Minimum password length
    #[serde(default = "default_min_password_length")]
    pub min_password_length: usize,

    /// Maximum password length
    #[serde(default = "default_max_password_length")]
    pub max_password_length: usize,
}

impl Default for UserPasswordConfig {
    fn default() -> Self {
        Self {
            min_password_length: 8,
            max_password_length: 128,
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
    /// Rate limiting settings
    #[serde(default)]
    pub rate_limit: RateLimitConfig,

    /// Maximum request body size in bytes
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,

    /// Security headers configuration
    #[serde(default)]
    pub headers: SecurityHeadersConfig,
}

/// Rate limit configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Requests per minute
    #[serde(default = "default_rate_limit_rpm")]
    pub rate_limit_rpm: u32,
}

/// Security headers configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityHeadersConfig {
    /// Whether security headers are enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// HSTS max age in seconds
    #[serde(default = "default_hsts_max_age")]
    pub hsts_max_age: u32,

    /// Whether to include subdomains in HSTS
    #[serde(default = "default_true")]
    pub hsts_include_subdomains: bool,

    /// X-Frame-Options value
    #[serde(default = "default_frame_options")]
    pub frame_options: String,

    /// X-Content-Type-Options value
    #[serde(default = "default_content_type_options")]
    pub content_type_options: String,

    /// Referrer-Policy value
    #[serde(default = "default_referrer_policy")]
    pub referrer_policy: String,

    /// Content Security Policy configuration
    #[serde(default)]
    pub csp: ContentSecurityPolicyConfig,
}

/// Content Security Policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentSecurityPolicyConfig {
    /// Whether CSP is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Default source directives
    #[serde(default = "default_csp_self")]
    pub default_src: Vec<String>,

    /// Script source directives
    #[serde(default = "default_csp_script_src")]
    pub script_src: Vec<String>,

    /// Style source directives
    #[serde(default = "default_csp_style_src")]
    pub style_src: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            rate_limit: RateLimitConfig::default(),
            max_body_size: default_max_body_size(),
            headers: SecurityHeadersConfig::default(),
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            rate_limit_rpm: default_rate_limit_rpm(),
        }
    }
}

impl Default for SecurityHeadersConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hsts_max_age: default_hsts_max_age(),
            hsts_include_subdomains: true,
            frame_options: default_frame_options(),
            content_type_options: default_content_type_options(),
            referrer_policy: default_referrer_policy(),
            csp: ContentSecurityPolicyConfig::default(),
        }
    }
}

impl Default for ContentSecurityPolicyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_src: default_csp_self(),
            script_src: default_csp_script_src(),
            style_src: default_csp_style_src(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_rate_limit_rpm() -> u32 {
    1000 // 1000 requests per minute
}

fn default_max_body_size() -> usize {
    1024 * 1024 // 1MB
}

fn default_hsts_max_age() -> u32 {
    31536000 // 1 year in seconds
}

fn default_frame_options() -> String {
    "DENY".to_string()
}

fn default_content_type_options() -> String {
    "nosniff".to_string()
}

fn default_referrer_policy() -> String {
    "strict-origin-when-cross-origin".to_string()
}

fn default_csp_self() -> Vec<String> {
    vec!["'self'".to_string()]
}

fn default_csp_script_src() -> Vec<String> {
    vec![
        "'self'".to_string(),
        "'unsafe-inline'".to_string(),
        "'unsafe-eval'".to_string(),
    ]
}

fn default_csp_style_src() -> Vec<String> {
    vec!["'self'".to_string(), "'unsafe-inline'".to_string()]
}

fn default_min_password_length() -> usize {
    8
}

fn default_max_password_length() -> usize {
    128
}

/// Development and testing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevelopmentConfig {
    /// Enable mock token endpoint for CI/testing
    #[serde(default)]
    pub enable_mock_auth: bool,

    /// Require authorization header for mock endpoint
    #[serde(default)]
    pub mock_auth_require_header: bool,

    /// Authorization header value required for mock endpoint
    #[serde(default)]
    pub mock_auth_header_value: Option<String>,
}

impl Default for DevelopmentConfig {
    fn default() -> Self {
        Self {
            enable_mock_auth: false,        // Disabled by default for security
            mock_auth_require_header: true, // Require auth header by default
            mock_auth_header_value: None,
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
