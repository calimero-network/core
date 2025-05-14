use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use calimero_auth::{AuthService, AuthProvider};
use calimero_auth::config::{load_config, default_config, AuthConfig};
use calimero_auth::providers::{jwt::TokenManager, near_wallet::NearWalletProvider};
use calimero_auth::service::start_server;
use calimero_auth::storage::create_storage;
use tracing::{info, level_filters::LevelFilter};
use tracing_subscriber::EnvFilter;

/// Calimero Authentication Service
#[derive(Parser, Debug)]
#[clap(version, about, long_about = None)]
struct Cli {
    /// Path to the configuration file
    #[clap(short, long, value_parser)]
    config: Option<PathBuf>,
    
    /// Bind address for the server
    #[clap(short, long, value_parser)]
    bind: Option<String>,
    
    /// Node URL to forward authenticated requests to
    #[clap(short, long, value_parser)]
    node_url: Option<String>,
    
    /// Authentication mode: "none" for development or "forward" for production
    #[clap(short = 'm', long, value_parser, default_value = "forward")]
    auth_mode: String,
    
    /// Enable verbose logging
    #[clap(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Parse command line arguments
    let cli = Cli::parse();
    
    // Initialize logging
    let filter = match cli.verbose {
        0 => EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info")),
        1 => EnvFilter::new("debug"),
        _ => EnvFilter::new("trace"),
    };
    
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();
    
    // Load configuration
    let mut config = if let Some(config_path) = cli.config {
        info!("Loading configuration from {}", config_path.display());
        load_config(config_path.to_str().unwrap())?
    } else {
        info!("Using default configuration");
        default_config()
    };
    
    // Override configuration with command line arguments
    if let Some(bind) = cli.bind {
        config.listen_addr = bind.parse()?;
    }
    
    if let Some(node_url) = cli.node_url {
        config.node_url = node_url;
    }
    
    // Check auth mode
    let auth_mode = cli.auth_mode.to_lowercase();
    if auth_mode != "none" && auth_mode != "forward" {
        return Err(eyre::eyre!("Invalid auth mode. Must be 'none' or 'forward'"));
    }
    
    // If auth mode is "none", skip initialization and just start the server
    if auth_mode == "none" {
        info!("Starting in development mode with no authentication");
        // Create empty auth service and storage
        let storage = create_storage(&config.storage).await?;
        let auth_service = AuthService::new(vec![]);
        
        // Start server
        info!("Starting auth server on {}", config.listen_addr);
        start_server(auth_service, storage, config).await?;
        return Ok(());
    }
    
    // Initialize storage
    let storage = create_storage(&config.storage).await?;
    
    // Create token manager for JWT tokens
    let token_manager = TokenManager::new(config.jwt.clone(), storage.clone());
    
    // Initialize providers
    let mut providers: Vec<Box<dyn AuthProvider>> = Vec::new();
    
    // Add NEAR wallet provider
    if config.providers.near_wallet {
        info!("Initializing NEAR wallet provider with JWT token generation");
        providers.push(Box::new(NearWalletProvider::with_token_manager(
            config.near.clone(), 
            storage.clone(),
            token_manager,
        )));
    }
    
    // Create auth service
    let auth_service = AuthService::new(providers);
    
    // Start server
    info!("Starting auth server on {}", config.listen_addr);
    start_server(auth_service, storage, config).await?;
    
    Ok(())
} 