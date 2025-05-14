use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Extension, Path, Query, Request};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{from_fn, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hex;
use rand::{thread_rng, Rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::signal;
use tower_http::cors::CorsLayer;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use tracing::{debug, error, info};

use crate::config::AuthConfig;
use crate::providers::jwt::TokenManager;
use crate::storage::{deserialize, prefixes, serialize, ClientKey, Permission, RootKey, Storage};
use crate::{AuthError, AuthProvider, AuthService};

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

    // Configure the CORS layer
    let cors_layer = if config.cors.allow_all_origins {
        CorsLayer::permissive()
    } else {
        let mut layer = CorsLayer::new();

        // Add allowed origins
        for origin in &config.cors.allowed_origins {
            layer = layer.allow_origin(origin.parse::<axum::http::HeaderValue>().unwrap());
        }

        // Add allowed methods
        let methods: Vec<axum::http::Method> = config
            .cors
            .allowed_methods
            .iter()
            .filter_map(|m| m.parse().ok())
            .collect();
        layer = layer.allow_methods(methods);

        // Add allowed headers
        layer = layer.allow_headers(
            config
                .cors
                .allowed_headers
                .iter()
                .filter_map(|h| h.parse::<axum::http::HeaderName>().ok())
                .collect::<Vec<_>>(),
        );

        layer
    };

    // Create the router
    let app: Router = Router::new()
        // Authentication endpoints
        .route("/auth/login", get(login_handler))
        .route("/auth/token", post(token_handler))
        .route("/auth/refresh", post(refresh_token_handler))
        .route("/auth/validate", post(validate_handler))
        .route("/auth/callback", get(callback_handler))
        .route("/auth/challenge", get(challenge_handler))
        // Root key management
        .route("/auth/keys", get(list_keys_handler))
        .route("/auth/keys", post(create_key_handler))
        .route("/auth/keys/:key_id", delete(delete_key_handler))
        // Client key management
        .route("/auth/keys/:key_id/clients", get(list_clients_handler))
        .route("/auth/keys/:key_id/clients", post(create_client_handler))
        .route(
            "/auth/keys/:key_id/clients/:client_id",
            delete(delete_client_handler),
        )
        // Permission management
        .route("/auth/permissions", get(list_permissions_handler))
        .route(
            "/auth/keys/:key_id/permissions",
            get(get_key_permissions_handler),
        )
        .route(
            "/auth/keys/:key_id/permissions",
            put(update_key_permissions_handler),
        )
        // Identity endpoint for development detection
        .route("/identity", get(identity_handler))
        // Apply CORS layer
        .layer(cors_layer)
        // Apply session layer
        .layer(session_layer)
        // Forward auth middleware for reverse proxy
        .layer(from_fn(forward_auth_middleware))
        // Add the state as Extension - this needs to be at the end
        .layer(Extension(Arc::clone(&state)));

    // Bind to the address
    let addr = config.listen_addr;
    info!("Auth service listening on {}", addr);

    // Start the server using Axum's built-in server
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

/// Wait for a shutdown signal
async fn shutdown_signal() {
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

/// Forward authentication middleware for reverse proxy
///
/// This middleware is used by reverse proxies to validate authentication.
/// If the request is authenticated, it adds the user ID and permissions to the response headers.
///
/// # Arguments
///
/// * `request` - The request to check
/// * `next` - The next middleware
///
/// # Returns
///
/// * `Result<Response, StatusCode>` - The response or error
pub async fn forward_auth_middleware(
    state: Extension<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // Skip authentication for login and token endpoints
    let path = request.uri().path();
    if path.starts_with("/auth/") {
        return Ok(next.run(request).await);
    }

    // Extract headers for token validation
    let headers = request.headers().clone();

    // Validate the request using the headers
    match state.auth_service.verify_token_from_headers(&headers).await {
        Ok(auth_response) => {
            if !auth_response.is_valid {
                return Err(StatusCode::UNAUTHORIZED);
            }

            // Continue with normal request
            let mut response = next.run(request).await;

            // Add authentication headers
            if let Some(key_id) = auth_response.key_id.as_ref() {
                response
                    .headers_mut()
                    .insert("X-Auth-User", key_id.parse().unwrap());
            }

            // Add permissions
            if !auth_response.permissions.is_empty() {
                response.headers_mut().insert(
                    "X-Auth-Permissions",
                    auth_response.permissions.join(",").parse().unwrap(),
                );
            }

            Ok(response)
        }
        Err(_) => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Login request handler
///
/// This endpoint serves the login page.
async fn login_handler(
    state: Extension<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    // Read the login template
    let html = include_str!("../templates/login.html");

    // In a real implementation, you would customize the HTML with any needed parameters

    (StatusCode::OK, [("Content-Type", "text/html")], html)
}

/// Token request
#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    /// Authentication method
    pub auth_method: String,
    /// Public key
    pub public_key: String,
    /// Wallet address (if applicable)
    pub wallet_address: Option<String>,
    /// Client name
    pub client_name: String,
    /// Permissions requested
    pub permissions: Option<Vec<String>>,
    /// Timestamp
    pub timestamp: u64,
    /// Signature
    pub signature: String,
    /// Message that was signed (only for NEAR wallet)
    pub message: Option<String>,
}

/// Token response
#[derive(Debug, Serialize)]
struct TokenResponse {
    /// Access token
    access_token: String,
    /// Refresh token
    refresh_token: String,
    /// Token type
    token_type: String,
    /// Expires in seconds
    expires_in: u64,
    /// Client ID
    client_id: String,
    /// Error information (if any)
    error: Option<String>,
}

/// Token handler
///
/// This endpoint generates JWT tokens for authenticated clients.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The token request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn token_handler(
    state: Extension<Arc<AppState>>,
    Json(token_request): Json<TokenRequest>,
) -> impl IntoResponse {
    // Special handling for NEAR wallet authentication
    if token_request.auth_method == "near_wallet" {
        // Find the NEAR wallet provider
        let near_provider = state
            .0
            .auth_service
            .providers()
            .iter()
            .find(|p| p.name() == "near_wallet")
            .ok_or_else(|| {
                AuthError::AuthenticationFailed("NEAR wallet provider not found".to_string())
            });

        if let Ok(provider) = near_provider {
            // Create headers to pass to the provider
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-near-account-id",
                token_request
                    .wallet_address
                    .clone()
                    .unwrap_or_default()
                    .parse()
                    .unwrap(),
            );
            headers.insert(
                "x-near-public-key",
                token_request.public_key.parse().unwrap(),
            );
            headers.insert("x-near-signature", token_request.signature.parse().unwrap());

            // Use as_ref() here to avoid moving the message
            headers.insert(
                "x-near-message",
                token_request
                    .message
                    .as_ref()
                    .unwrap_or(&String::new())
                    .parse()
                    .unwrap(),
            );

            // Build a request with the headers
            let mut req = Request::builder()
                .method("POST")
                .uri("/auth/token")
                .body(Body::empty())
                .unwrap();

            *req.headers_mut() = headers;

            // Verify the request using the provider
            match provider.verify_request(&req) {
                Ok(verifier) => {
                    match verifier.verify().await {
                        Ok(auth_response) => {
                            if !auth_response.is_valid {
                                return (
                                    StatusCode::UNAUTHORIZED,
                                    Json(serde_json::json!({
                                        "error": "Authentication failed"
                                    })),
                                )
                                    .into_response();
                            }

                            // If authentication successful, generate new tokens
                            if let Some(key_id) = auth_response.key_id.as_ref() {
                                // Generate a client ID
                                let client_id = token_request.client_name.clone();

                                match state
                                    .0
                                    .token_generator
                                    .generate_token_pair(
                                        &client_id,
                                        key_id,
                                        &auth_response.permissions,
                                    )
                                    .await
                                {
                                    Ok((access_token, refresh_token)) => {
                                        let response = TokenResponse {
                                            access_token,
                                            refresh_token,
                                            token_type: "Bearer".to_string(),
                                            expires_in: state.0.config.jwt.access_token_expiry,
                                            client_id,
                                            error: None,
                                        };

                                        return (StatusCode::OK, Json(response)).into_response();
                                    }
                                    Err(err) => {
                                        error!("Failed to generate tokens: {}", err);
                                        return (
                                            StatusCode::INTERNAL_SERVER_ERROR,
                                            Json(serde_json::json!({
                                                "error": "Failed to generate tokens"
                                            })),
                                        )
                                            .into_response();
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            error!("Authentication failed: {}", err);
                            return (
                                StatusCode::UNAUTHORIZED,
                                Json(serde_json::json!({
                                    "error": format!("Authentication failed: {}", err)
                                })),
                            )
                                .into_response();
                        }
                    }
                }
                Err(err) => {
                    error!("Failed to verify request: {}", err);
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "error": format!("Failed to verify request: {}", err)
                        })),
                    )
                        .into_response();
                }
            }
        }
    }

    // Original implementation for other auth methods
    // Attempt to authenticate the request based on the token request
    match state
        .0
        .auth_service
        .authenticate_token_request(&token_request)
        .await
    {
        Ok(auth_response) => {
            if !auth_response.is_valid {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "error": "Authentication failed"
                    })),
                )
                    .into_response();
            }

            // If authentication successful, generate new tokens
            if let Some(key_id) = auth_response.key_id.as_ref() {
                // Generate a client ID
                let client_id = token_request.client_name.clone();

                match state
                    .0
                    .token_generator
                    .generate_token_pair(&client_id, key_id, &auth_response.permissions)
                    .await
                {
                    Ok((access_token, refresh_token)) => {
                        let response = TokenResponse {
                            access_token,
                            refresh_token,
                            token_type: "Bearer".to_string(),
                            expires_in: state.0.config.jwt.access_token_expiry,
                            client_id,
                            error: None,
                        };

                        return (StatusCode::OK, Json(response)).into_response();
                    }
                    Err(err) => {
                        error!("Failed to generate tokens: {}", err);
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "error": "Failed to generate tokens"
                            })),
                        )
                            .into_response();
                    }
                }
            }

            // If no key ID is available, token generation failed
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to generate tokens: no key ID available"
                })),
            )
                .into_response();
        }
        Err(err) => {
            error!("Authentication failed: {}", err);
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": format!("Authentication failed: {}", err)
                })),
            )
                .into_response();
        }
    }
}

/// Refresh token request
#[derive(Debug, Deserialize)]
struct RefreshTokenRequest {
    /// Refresh token
    refresh_token: String,
}

/// Refresh token handler
///
/// This endpoint refreshes an access token using a refresh token.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The refresh token request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn refresh_token_handler(
    state: Extension<Arc<AppState>>,
    Json(request): Json<RefreshTokenRequest>,
) -> impl IntoResponse {
    match state
        .0
        .token_generator
        .refresh_token_pair(&request.refresh_token)
        .await
    {
        Ok((access_token, refresh_token)) => {
            let response = TokenResponse {
                access_token,
                refresh_token,
                token_type: "Bearer".to_string(),
                expires_in: state.0.config.jwt.access_token_expiry,
                client_id: "client_123456".to_string(), // This should come from the token
                error: None,
            };

            (StatusCode::OK, Json(response))
        }
        Err(err) => {
            debug!("Failed to refresh token: {}", err);

            // Use the same response type but with error info
            let error_response = TokenResponse {
                access_token: String::new(),
                refresh_token: String::new(),
                token_type: String::new(),
                expires_in: 0,
                client_id: String::new(),
                error: Some("Invalid refresh token".to_string()),
            };

            (StatusCode::UNAUTHORIZED, Json(error_response))
        }
    }
}

/// Validation handler
///
/// This endpoint validates a request and returns authentication information.
/// It's used by reverse proxies for forward authentication.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The request to validate
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn validate_handler(
    state: Extension<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Validate the request using the headers
    match state
        .0
        .auth_service
        .verify_token_from_headers(&headers)
        .await
    {
        Ok(auth_response) => {
            if !auth_response.is_valid {
                return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
            }

            // Return success with appropriate headers
            let mut response_headers = HeaderMap::new();

            if let Some(key_id) = auth_response.key_id.as_ref() {
                response_headers.insert("X-Auth-User", key_id.parse().unwrap());
            }

            // Add permissions as a comma-separated list
            if !auth_response.permissions.is_empty() {
                response_headers.insert(
                    "X-Auth-Permissions",
                    auth_response.permissions.join(",").parse().unwrap(),
                );
            }

            // Convert to Response to match error case
            (StatusCode::OK, response_headers, "").into_response()
        }
        Err(_) => (StatusCode::UNAUTHORIZED, HeaderMap::new(), "Unauthorized").into_response(),
    }
}

/// OAuth callback handler
///
/// This endpoint handles callbacks from OAuth providers.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn callback_handler(_state: Extension<Arc<AppState>>) -> impl IntoResponse {
    // This is a placeholder implementation
    // In a real implementation, you would:
    // 1. Extract the code from the request
    // 2. Exchange it for tokens
    // 3. Validate the tokens
    // 4. Create or lookup the root key
    // 5. Create a client key
    // 6. Generate tokens
    // 7. Redirect to the original URL with the tokens

    (
        StatusCode::OK,
        "OAuth callback - implement with your OAuth provider",
    )
}

/// Key list handler
///
/// This endpoint lists all root keys.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn list_keys_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    // List all keys with the root key prefix
    match state.0.storage.list_keys(prefixes::ROOT_KEY).await {
        Ok(keys) => {
            let mut root_keys = Vec::new();

            for key in keys {
                // Skip the prefix
                let key_id = key.strip_prefix(prefixes::ROOT_KEY).unwrap_or(&key);

                // Get the key data
                if let Ok(Some(data)) = state.0.storage.get(&key).await {
                    if let Ok(root_key) = deserialize::<RootKey>(&data) {
                        root_keys.push(serde_json::json!({
                            "key_id": key_id,
                            "public_key": root_key.public_key,
                            "auth_method": root_key.auth_method,
                            "created_at": root_key.created_at,
                            "revoked_at": root_key.revoked_at,
                            "last_used_at": root_key.last_used_at,
                        }));
                    }
                }
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "keys": root_keys
                })),
            )
        }
        Err(err) => {
            error!("Failed to list keys: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to list keys"
                })),
            )
        }
    }
}

/// Key creation request
#[derive(Debug, Deserialize)]
struct CreateKeyRequest {
    /// Public key
    public_key: String,
    /// Authentication method
    auth_method: String,
    /// Wallet address (if applicable)
    wallet_address: Option<String>,
}

/// Key creation handler
///
/// This endpoint creates a new root key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The key creation request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn create_key_handler(
    state: Extension<Arc<AppState>>,
    Json(request): Json<CreateKeyRequest>,
) -> impl IntoResponse {
    // Create a hash of the public key to use as the key ID
    let mut hasher = Sha256::new();
    hasher.update(request.public_key.as_bytes());
    let hash = hasher.finalize();
    let key_id = hex::encode(hash);

    // Create the root key
    let root_key = RootKey {
        public_key: request.public_key,
        auth_method: request.auth_method,
        created_at: chrono::Utc::now().timestamp() as u64,
        revoked_at: None,
        last_used_at: None,
    };

    // Store the root key
    let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
    match serialize(&root_key) {
        Ok(data) => match state.0.storage.set(&key, &data).await {
            Ok(_) => (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "key_id": key_id,
                    "public_key": root_key.public_key,
                    "auth_method": root_key.auth_method,
                    "created_at": root_key.created_at,
                })),
            ),
            Err(err) => {
                error!("Failed to store root key: {}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "Failed to store root key"
                    })),
                )
            }
        },
        Err(err) => {
            error!("Failed to serialize root key: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to serialize root key"
                })),
            )
        }
    }
}

/// Key deletion handler
///
/// This endpoint revokes a root key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The key ID to delete
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn delete_key_handler(
    state: Extension<Arc<AppState>>,
    Path(key_id): Path<String>,
) -> impl IntoResponse {
    // Get the key
    let key = format!("{}{}", prefixes::ROOT_KEY, key_id);

    match state.0.storage.get(&key).await {
        Ok(Some(data)) => {
            let mut root_key: RootKey = match deserialize(&data) {
                Ok(key) => key,
                Err(err) => {
                    error!("Failed to deserialize root key: {}", err);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Failed to deserialize root key"
                        })),
                    );
                }
            };

            // Mark the key as revoked
            root_key.revoked_at = Some(chrono::Utc::now().timestamp() as u64);

            // Store the updated key
            match serialize(&root_key) {
                Ok(data) => match state.0.storage.set(&key, &data).await {
                    Ok(_) => (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "key_id": key_id,
                            "revoked_at": root_key.revoked_at,
                        })),
                    ),
                    Err(err) => {
                        error!("Failed to update root key: {}", err);
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "error": "Failed to update root key"
                            })),
                        )
                    }
                },
                Err(err) => {
                    error!("Failed to serialize root key: {}", err);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Failed to serialize root key"
                        })),
                    )
                }
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Root key not found"
            })),
        ),
        Err(err) => {
            error!("Failed to get root key: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to get root key"
                })),
            )
        }
    }
}

/// Client list handler
///
/// This endpoint lists all client keys for a root key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The root key ID
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn list_clients_handler(
    state: Extension<Arc<AppState>>,
    Path(key_id): Path<String>,
) -> impl IntoResponse {
    // List all client keys with the root_clients prefix
    let prefix = format!("{}{}", prefixes::ROOT_CLIENTS, key_id);

    match state.0.storage.list_keys(&prefix).await {
        Ok(keys) => {
            let mut client_keys = Vec::new();

            for key in keys {
                // Get the client ID
                let client_id = key.strip_prefix(&prefix).unwrap_or(&key);

                // Get the client key data
                let client_key = format!("{}{}", prefixes::CLIENT_KEY, client_id);

                if let Ok(Some(data)) = state.0.storage.get(&client_key).await {
                    if let Ok(client_key) = deserialize::<ClientKey>(&data) {
                        client_keys.push(serde_json::json!({
                            "client_id": client_id,
                            "root_key_id": client_key.root_key_id,
                            "name": client_key.name,
                            "permissions": client_key.permissions,
                            "created_at": client_key.created_at,
                            "expires_at": client_key.expires_at,
                            "revoked_at": client_key.revoked_at,
                        }));
                    }
                }
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "clients": client_keys
                })),
            )
        }
        Err(err) => {
            error!("Failed to list client keys: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to list client keys"
                })),
            )
        }
    }
}

/// Client creation request
#[derive(Debug, Deserialize)]
struct CreateClientRequest {
    /// Client name
    name: String,
    /// Permissions
    permissions: Vec<String>,
    /// Expiration time (in seconds from now)
    expires_in: Option<u64>,
}

/// Client creation handler
///
/// This endpoint creates a new client key for a root key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The root key ID
/// * `request` - The client creation request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn create_client_handler(
    state: Extension<Arc<AppState>>,
    Path(key_id): Path<String>,
    Json(request): Json<CreateClientRequest>,
) -> impl IntoResponse {
    // Check if the root key exists
    let root_key_key = format!("{}{}", prefixes::ROOT_KEY, key_id);

    match state.0.storage.get(&root_key_key).await {
        Ok(Some(_)) => {
            // Generate a client ID
            let client_id = format!(
                "client_{}",
                uuid::Uuid::new_v4().to_string().replace("-", "")
            );

            // Calculate expiration time
            let expires_at = request
                .expires_in
                .map(|secs| chrono::Utc::now().timestamp() as u64 + secs);

            // Create the client key
            let client_key = ClientKey {
                client_id: client_id.clone(),
                root_key_id: key_id.clone(),
                name: request.name,
                permissions: request.permissions,
                created_at: chrono::Utc::now().timestamp() as u64,
                expires_at,
                revoked_at: None,
            };

            // Store the client key
            let client_key_key = format!("{}{}", prefixes::CLIENT_KEY, client_id);

            match serialize(&client_key) {
                Ok(data) => {
                    match state.0.storage.set(&client_key_key, &data).await {
                        Ok(_) => {
                            // Create the index from root key to client key
                            let index_key = format!("{}{}", prefixes::ROOT_CLIENTS, key_id);
                            let _ = state.0.storage.set(&index_key, &client_id.as_bytes()).await;

                            (
                                StatusCode::CREATED,
                                Json(serde_json::json!({
                                    "client_id": client_id,
                                    "root_key_id": key_id,
                                    "name": client_key.name,
                                    "permissions": client_key.permissions,
                                    "created_at": client_key.created_at,
                                    "expires_at": client_key.expires_at,
                                })),
                            )
                        }
                        Err(err) => {
                            error!("Failed to store client key: {}", err);
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({
                                    "error": "Failed to store client key"
                                })),
                            )
                        }
                    }
                }
                Err(err) => {
                    error!("Failed to serialize client key: {}", err);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Failed to serialize client key"
                        })),
                    )
                }
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Root key not found"
            })),
        ),
        Err(err) => {
            error!("Failed to get root key: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to get root key"
                })),
            )
        }
    }
}

/// Client deletion handler
///
/// This endpoint revokes a client key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The root key ID
/// * `client_id` - The client ID to delete
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn delete_client_handler(
    state: Extension<Arc<AppState>>,
    Path((key_id, client_id)): Path<(String, String)>,
) -> impl IntoResponse {
    // Get the client key
    let client_key_key = format!("{}{}", prefixes::CLIENT_KEY, client_id);

    match state.0.storage.get(&client_key_key).await {
        Ok(Some(data)) => {
            let mut client_key: ClientKey = match deserialize(&data) {
                Ok(key) => key,
                Err(err) => {
                    error!("Failed to deserialize client key: {}", err);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Failed to deserialize client key"
                        })),
                    );
                }
            };

            // Check if the client key belongs to the specified root key
            if client_key.root_key_id != key_id {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "Client key does not belong to the specified root key"
                    })),
                );
            }

            // Mark the client key as revoked
            client_key.revoked_at = Some(chrono::Utc::now().timestamp() as u64);

            // Store the updated client key
            match serialize(&client_key) {
                Ok(data) => match state.0.storage.set(&client_key_key, &data).await {
                    Ok(_) => (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "client_id": client_id,
                            "revoked_at": client_key.revoked_at,
                        })),
                    ),
                    Err(err) => {
                        error!("Failed to update client key: {}", err);
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "error": "Failed to update client key"
                            })),
                        )
                    }
                },
                Err(err) => {
                    error!("Failed to serialize client key: {}", err);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Failed to serialize client key"
                        })),
                    )
                }
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Client key not found"
            })),
        ),
        Err(err) => {
            error!("Failed to get client key: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to get client key"
                })),
            )
        }
    }
}

/// Permission list handler
///
/// This endpoint lists all available permissions.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn list_permissions_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    // List all permissions with the permission prefix
    match state.0.storage.list_keys(prefixes::PERMISSION).await {
        Ok(keys) => {
            let mut permissions = Vec::new();

            for key in keys {
                // Get the permission data
                if let Ok(Some(data)) = state.0.storage.get(&key).await {
                    if let Ok(permission) = deserialize::<Permission>(&data) {
                        permissions.push(serde_json::json!({
                            "permission_id": permission.permission_id,
                            "name": permission.name,
                            "description": permission.description,
                            "resource_type": permission.resource_type,
                        }));
                    }
                }
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "permissions": permissions
                })),
            )
        }
        Err(err) => {
            error!("Failed to list permissions: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to list permissions"
                })),
            )
        }
    }
}

/// Key permissions handler
///
/// This endpoint gets the permissions for a root key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The root key ID
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn get_key_permissions_handler(
    _state: Extension<Arc<AppState>>,
    Path(_key_id): Path<String>,
) -> impl IntoResponse {
    // In a real implementation, you would look up the permissions for the key
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "permissions": ["admin"]
        })),
    )
}

/// Key permissions update request
#[derive(Debug, Deserialize)]
struct UpdateKeyPermissionsRequest {
    /// Permissions to add
    add: Option<Vec<String>>,
    /// Permissions to remove
    remove: Option<Vec<String>>,
}

/// Key permissions update handler
///
/// This endpoint updates the permissions for a root key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The root key ID
/// * `request` - The permissions update request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn update_key_permissions_handler(
    _state: Extension<Arc<AppState>>,
    Path(_key_id): Path<String>,
    Json(_request): Json<UpdateKeyPermissionsRequest>,
) -> impl IntoResponse {
    // In a real implementation, you would update the permissions for the key
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "permissions": ["admin"]
        })),
    )
}

/// Identity response
#[derive(Debug, Serialize)]
struct IdentityResponse {
    /// Node ID
    node_id: String,
    /// Version
    version: String,
    /// Authentication mode
    authentication_mode: String,
}

/// Identity handler
///
/// This endpoint returns information about the node identity.
/// It's used by clients to detect authentication mode.
///
/// # Arguments
///
/// * `state` - The application state (optional)
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn identity_handler(state: Option<Extension<Arc<AppState>>>) -> impl IntoResponse {
    // Determine the authentication mode based on the number of providers (or standalone mode)
    let auth_mode = match &state {
        Some(state) if !state.0.auth_service.providers().is_empty() => "forward",
        _ => "none",
    };

    // Create a node ID using a timestamp instead of UUID
    let node_id = match &state {
        Some(state) if !state.0.config.node_url.is_empty() => state.0.config.node_url.clone(),
        _ => format!("auth-node-{}", chrono::Utc::now().timestamp()),
    };

    let response = IdentityResponse {
        node_id,
        version: env!("CARGO_PKG_VERSION").to_string(),
        authentication_mode: auth_mode.to_string(),
    };

    (StatusCode::OK, Json(response))
}

/// Challenge request
#[derive(Debug, Deserialize)]
struct ChallengeRequest {
    provider: String,
    redirect_uri: Option<String>,
    client_id: Option<String>,
}

/// Challenge response
#[derive(Debug, Serialize)]
struct ChallengeResponse {
    message: String,
    timestamp: u64,
    network: String,
    rpc_url: String,
    wallet_url: String,
    redirect_uri: String,
}

/// Generate a random challenge
fn generate_random_challenge() -> String {
    let mut rng = thread_rng();
    let random_bytes: Vec<u8> = (0..32).map(|_| rng.gen::<u8>()).collect();
    STANDARD.encode(random_bytes)
}

/// Challenge handler
///
/// This endpoint generates a challenge for authentication.
///
/// # Arguments
///
/// * `state` - The application state
/// * `params` - The challenge request parameters
///
/// # Returns
///
/// * `impl IntoResponse` - The response
async fn challenge_handler(
    state: Extension<Arc<AppState>>,
    Query(params): Query<ChallengeRequest>,
) -> impl IntoResponse {
    // Only process NEAR wallet challenges for now
    if params.provider != "near_wallet" && params.provider != "near" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Unsupported provider"
            })),
        )
            .into_response();
    }

    // Generate a new challenge
    let timestamp = chrono::Utc::now().timestamp() as u64;
    let message = format!(
        "Calimero Authentication Request {}:{}",
        timestamp,
        generate_random_challenge()
    );

    // Get the redirect URI
    let redirect_uri = params.redirect_uri.unwrap_or_else(|| "/".to_string());

    // Create the response
    let response = ChallengeResponse {
        message,
        timestamp,
        network: state.0.config.near.network.clone(),
        rpc_url: state.0.config.near.rpc_url.clone(),
        wallet_url: state.0.config.near.wallet_url.clone(),
        redirect_uri,
    };

    (StatusCode::OK, Json(response)).into_response()
}
