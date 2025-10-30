use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use eyre::Result;
use mero_auth::auth::token::TokenManager;
use mero_auth::config::{
    load_config, AuthConfig, ContentSecurityPolicyConfig, DevelopmentConfig, JwtConfig,
    NearWalletConfig, SecurityConfig, SecurityHeadersConfig, StorageConfig, UserPasswordConfig,
};
use mero_auth::secrets::SecretManager;
use mero_auth::server::{shutdown_signal, start_server};
use mero_auth::storage::{create_storage, Storage};
use mero_auth::{providers, AuthService};
use tracing::{info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Calimero Authentication Service
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// Path to the configuration file
    #[clap(short, long, value_parser)]
    config: Option<PathBuf>,

    /// Bind address for the server
    #[clap(short, long, value_parser)]
    bind: Option<String>,

    /// Enable verbose logging (can be specified multiple times)
    #[clap(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

/// Create a default configuration for when no config file is provided
fn create_default_config() -> AuthConfig {
    let mut providers = HashMap::new();
    providers.insert("near_wallet".to_string(), true);

    AuthConfig {
        listen_addr: "127.0.0.1:3001".parse().unwrap(),
        jwt: JwtConfig {
            issuer: "calimero-auth".to_string(),
            access_token_expiry: 3600,
            refresh_token_expiry: 2592000,
        },
        storage: StorageConfig::Memory,
        cors: Default::default(),
        security: SecurityConfig {
            max_body_size: 1024 * 1024, // 1MB
            headers: SecurityHeadersConfig {
                enabled: true,
                hsts_max_age: 31536000, // 1 year
                hsts_include_subdomains: true,
                frame_options: "DENY".to_string(),
                content_type_options: "nosniff".to_string(),
                referrer_policy: "strict-origin-when-cross-origin".to_string(),
                csp: ContentSecurityPolicyConfig {
                    enabled: true,
                    default_src: vec!["'self'".to_string()],
                    script_src: vec![
                        "'self'".to_string(),
                        "'unsafe-inline'".to_string(),
                        "'unsafe-eval'".to_string(),
                    ],
                    style_src: vec!["'self'".to_string(), "'unsafe-inline'".to_string()],
                },
            },
        },
        providers,
        near: NearWalletConfig::default(),
        user_password: UserPasswordConfig::default(),
        development: DevelopmentConfig::default(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let cli = Cli::parse();

    // Initialize logging
    let filter = match cli.verbose {
        0 => tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "calimero_auth=info,tower_http=debug".into()),
        1 => tracing_subscriber::EnvFilter::new("debug"),
        _ => tracing_subscriber::EnvFilter::new("trace"),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load configuration
    let mut config = if let Some(config_path) = &cli.config {
        info!("Loading configuration from {}", config_path.display());
        match load_config(config_path.to_str().unwrap()) {
            Ok(config) => config,
            Err(err) => {
                warn!("Failed to load configuration: {}", err);
                warn!("Using default configuration instead");
                create_default_config()
            }
        }
    } else {
        info!("Using default configuration");
        create_default_config()
    };

    // Override configuration with command line arguments
    if let Some(bind) = cli.bind {
        config.listen_addr = bind.parse()?;
    }

    // Create the storage backend
    let storage = create_storage(&config.storage)
        .await
        .expect("Failed to create storage");

    // Create the secret manager with the storage trait
    let secret_manager = Arc::new(SecretManager::new(storage.clone() as Arc<dyn Storage>));
    secret_manager
        .initialize()
        .await
        .expect("Failed to initialize secret manager");

    // Create JWT token manager
    let token_manager = TokenManager::new(config.jwt.clone(), storage.clone(), secret_manager);

    // Create providers using the provider factory
    info!("Starting authentication service");
    let providers = providers::create_providers(storage.clone(), &config, token_manager.clone())
        .expect("Failed to create authentication providers");

    info!("Initialized {} authentication providers", providers.len());
    for provider in &providers {
        info!("  - {} ({})", provider.name(), provider.description());
    }

    let auth_service = AuthService::new(providers, token_manager);

    // Start the server
    info!("Starting auth server on {}", config.listen_addr);

    tokio::select! {
        result = start_server(auth_service, storage, config) => {
            if let Err(err) = result {
                eprintln!("Server error: {err}");
                return Err(err);
            }
        }
        _ = shutdown_signal() => {
            info!("Shutdown signal received, shutting down");
        }
    }

    Ok(())
}
