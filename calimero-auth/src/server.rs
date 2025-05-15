use std::sync::Arc;

use axum::{
    Extension,
    Router,
    Server,
};
use std::net::SocketAddr;

use crate::api::routes;
use crate::auth::middleware::AuthMiddleware;
use crate::auth::token::service::TokenService;
use crate::config::AuthConfig;
use crate::providers::jwt::TokenManager;
use crate::auth::AuthService;
use crate::storage::Storage;

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

    // Create the token service for JWT operations
    let token_service = Arc::new(TokenService::new(
        config.jwt.clone(),
        storage.clone(),
    ));

    // Create the authentication middleware
    let auth_middleware = Arc::new(AuthMiddleware::new(token_service.clone()));

    // Set up the router with all routes
    let app = routes::create_router()
        .layer(Extension(Arc::new(auth_service)))
        .layer(Extension(token_service))
        .layer(Extension(auth_middleware))
        .layer(Extension(config.clone()));

    // Get the bind address from configuration
    let addr = SocketAddr::from(([0, 0, 0, 0], config.server.port));

    // Start the server
    println!("Starting auth server on {}", addr);
    
    Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .map_err(|e| eyre::eyre!("Server error: {}", e))?;

    Ok(())
} 