//! mero-kms-phala: Key management service for merod nodes running in Phala Cloud TEE.
//!
//! This service validates TDX attestations from merod nodes and releases deterministic
//! storage encryption keys based on peer ID using Phala's dstack key derivation.

mod handlers;

use std::net::SocketAddr;

use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::handlers::create_router;

/// Configuration for the key releaser service.
#[derive(Debug, Clone)]
pub struct Config {
    /// Socket address to listen on.
    pub listen_addr: SocketAddr,
    /// Path to the dstack Unix socket.
    pub dstack_socket_path: String,
    /// Whether to accept mock attestations (for development only).
    pub accept_mock_attestation: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: SocketAddr::from(([0, 0, 0, 0], 8080)),
            dstack_socket_path: "/var/run/dstack.sock".to_string(),
            accept_mock_attestation: false,
        }
    }
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        let listen_addr = std::env::var("LISTEN_ADDR")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 8080)));

        let dstack_socket_path = std::env::var("DSTACK_SOCKET_PATH")
            .unwrap_or_else(|_| "/var/run/dstack.sock".to_string());

        let accept_mock_attestation = std::env::var("ACCEPT_MOCK_ATTESTATION")
            .map(|v| v.to_lowercase() == "true" || v == "1")
            .unwrap_or(false);

        Self {
            listen_addr,
            dstack_socket_path,
            accept_mock_attestation,
        }
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .with_level(true)
        .init();

    // Load configuration
    let config = Config::from_env();

    info!("Starting mero-kms-phala");
    info!("Listen address: {}", config.listen_addr);
    info!("Dstack socket: {}", config.dstack_socket_path);
    info!(
        "Accept mock attestation: {}",
        config.accept_mock_attestation
    );

    if config.accept_mock_attestation {
        tracing::warn!(
            "WARNING: Mock attestation acceptance is enabled. This should NEVER be used in production!"
        );
    }

    // Create router with handlers
    let app = create_router(config.clone())
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );

    // Start server
    let listener = tokio::net::TcpListener::bind(config.listen_addr).await?;
    info!("Server listening on {}", config.listen_addr);

    axum::serve(listener, app).await?;

    Ok(())
}
