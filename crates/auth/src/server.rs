use std::sync::Arc;

use tokio::signal;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use tracing::info;

use crate::auth::token::TokenManager;
use crate::api::routes::create_router;
use crate::config::AuthConfig;
use crate::storage::Storage;
use crate::AuthService;

/// Application state
pub struct AppState {
    /// Authentication service
    pub auth_service: AuthService,
    /// Storage backend
    pub storage: Arc<dyn Storage>,
    /// Token generator
    pub token_generator: TokenManager,
    /// Configuration
    pub config: AuthConfig,
}

/// Start the authentication service
///
/// # Arguments
///
/// * `auth_service` - The authentication service
/// * `storage` - The storage backend
/// * `config` - The configuration
///
/// # Returns
///
/// * `Result<(), eyre::Error>` - Success or error
pub async fn start_server(
    auth_service: AuthService,
    storage: Arc<dyn Storage>,
    config: AuthConfig,
) -> eyre::Result<()> {
    let token_generator = TokenManager::new(config.jwt.clone(), storage.clone());

    // Create the application state
    let state = Arc::new(AppState {
        auth_service,
        storage,
        token_generator,
        config: config.clone(),
    });

    // Create the session store
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);

    // Create the router with all routes and middleware
    let app = create_router(Arc::clone(&state), &config)
        // Apply session layer
        .layer(session_layer);

    // Bind to the address
    let addr = config.listen_addr;
    info!("Auth service listening on {}", addr);

    // Start the server using Axum's built-in server
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

/// Wait for a shutdown signal
pub async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received, shutting down");
} 