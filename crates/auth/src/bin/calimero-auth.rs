use std::path::PathBuf;

use clap::Parser;
use eyre::Result;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use calimero_auth::{
    config::{default_config, load_config, AuthConfig},
    providers,
    server::{shutdown_signal, start_server},
    storage::create_storage,
    AuthService,
};

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

    /// Node URL to forward authenticated requests to
    #[clap(short, long, value_parser)]
    node_url: Option<String>,

    /// Authentication mode: "none" for development mode with no authentication
    #[clap(short = 'm', long, value_parser, default_value = "forward")]
    auth_mode: String,

    /// Enable verbose logging (can be specified multiple times)
    #[clap(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
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
                default_config()
            }
        }
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

    // Create the storage backend
    let storage = create_storage(&config.storage).await
        .expect("Failed to create storage");

    // Check auth mode
    let auth_mode = cli.auth_mode.to_lowercase();
    if auth_mode != "none" && auth_mode != "forward" {
        return Err(eyre::eyre!(
            "Invalid auth mode. Must be 'none' or 'forward'"
        ));
    }

    // Create the authentication service
    let auth_service = if auth_mode == "none" {
        info!("Starting in development mode with no authentication");
        // Create empty auth service with no providers
        AuthService::new(vec![])
    } else {
        // Create providers using the provider factory
        info!("Starting in production mode with authentication");
        let providers = providers::create_providers(storage.clone(), &config)
            .expect("Failed to create authentication providers");
            
        info!("Initialized {} authentication providers", providers.len());
        for provider in &providers {
            info!("  - {} ({})", provider.name(), provider.description());
        }
        
        AuthService::new(providers)
    };

    // Start the server
    info!("Starting auth server on {}", config.listen_addr);
    
    tokio::select! {
        result = start_server(auth_service, storage, config) => {
            if let Err(err) = result {
                eprintln!("Server error: {}", err);
                return Err(err);
            }
        }
        _ = shutdown_signal() => {
            info!("Shutdown signal received, shutting down");
        }
    }

    Ok(())
}
