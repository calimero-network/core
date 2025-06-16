use std::sync::Arc;

use tokio::signal;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use tracing::info;

// use crate::api::routes::create_router;
// use crate::auth::token::TokenManager;
use crate::config::AuthConfig;
use crate::storage::{KeyManager, Storage};
use crate::utils::AuthMetrics;
use crate::AuthService;

/// Application state
pub struct AppState {
    /// Authentication service
    pub auth_service: AuthService,
    /// Storage backend
    pub storage: Arc<dyn Storage>,
    /// Key manager for domain operations
    pub key_manager: KeyManager,
    /// Token generator
    // pub token_generator: TokenManager,
    /// Configuration
    pub config: AuthConfig,
    /// Metrics
    pub metrics: AuthMetrics,
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
    let metrics = AuthMetrics::new();
    let key_manager = KeyManager::new(Arc::clone(&storage));

    // Create the application state
    let state = Arc::new(AppState {
        auth_service: auth_service.clone(),
        storage,
        key_manager,
        // token_generator: auth_service.get_token_manager().clone(),
        config: config.clone(),
        metrics,
    });

    // Create the session store
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);

    // // Create the router with all routes and middleware
    // let app = create_router(Arc::clone(&state), &config)
    //     // Apply session layer
    //     .layer(session_layer);

    // Bind to the address
    let addr = config.listen_addr;
    info!("Auth service listening on {}", addr);
    // info!(
    //     "Using {} auth provider(s)",
    //     state.auth_service.providers().len()
    // );

    // Start the server using Axum's built-in server
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // axum::serve(listener, app.into_make_service()).await?;

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
