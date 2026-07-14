use std::sync::Arc;

use axum::middleware::from_fn;
use axum::routing::{delete, get, post};
use axum::{Extension, Router};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::CorsLayer;

use super::handlers::client_keys::generate_client_key_handler;
#[cfg(debug_assertions)]
use crate::api::handlers::auth::mock_token_handler;
use crate::api::handlers::auth::{
    callback_handler, challenge_handler, login_handler, refresh_token_handler,
    revoke_token_handler, token_handler, validate_handler,
};
use crate::api::handlers::client_keys::{delete_client_handler, list_clients_handler};
use crate::api::handlers::permissions::{
    get_key_permissions_handler, update_key_permissions_handler,
};
use crate::api::handlers::root_keys::{create_key_handler, delete_key_handler, list_keys_handler};
use crate::api::handlers::{
    asset_handler, health_handler, identity_handler, metrics_handler, providers_handler,
};
use crate::auth::middleware::auth_middleware;
use crate::auth::security::{create_body_limit_layer, create_security_headers};
use crate::config::AuthConfig;
use crate::server::AppState;

/// Creates and configures the router with all routes and middleware
pub fn create_router(state: Arc<AppState>, config: &AuthConfig) -> Router {
    // Configure CORS layer
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

        // **Critical:** a browser hides every response header from JS unless the
        // server names it in `Access-Control-Expose-Headers`. `x-auth-error` is
        // load-bearing on the client: SDKs auto-refresh only on `token_expired`,
        // and this PR's single-use enforcement signals a revoked family with
        // `token_reuse`. Without this, a cross-origin browser client sees a bare
        // 401 — it can neither refresh nor recognise a revoked family, so it
        // retries forever. `CorsLayer::permissive()` (the allow-all branch
        // above) already exposes everything; only this explicit-origin branch
        // needed it. `mero-server` has had the same list, and tests pinning it,
        // since a prior production incident — see crates/server/src/lib.rs.
        layer = layer.expose_headers([
            axum::http::HeaderName::from_static("x-auth-error"),
            axum::http::HeaderName::from_static("x-auth-user"),
            axum::http::HeaderName::from_static("x-auth-permissions"),
        ]);

        layer
    };

    // 1. Public routes (no JWT validation required)
    let public_routes = Router::new()
        // Auth UI/Frontend
        .route("/login", get(login_handler)) // Main auth UI entry point
        .route("/assets/*path", get(asset_handler)) // Static assets for the UI
        .route("/favicon.ico", get(asset_handler)) // Favicon
        // Public Auth API endpoints
        .route("/token", post(token_handler))
        .route("/challenge", get(challenge_handler))
        .route("/callback", get(callback_handler))
        .route("/providers", get(providers_handler))
        .route("/health", get(health_handler))
        .route("/refresh", post(refresh_token_handler))
        .route("/validate", get(validate_handler).post(validate_handler));

    // Mock token endpoint is only compiled and registered in debug builds.
    #[cfg(debug_assertions)]
    let public_routes = public_routes.route("/mock-token", post(mock_token_handler));

    // 2. Protected routes (require JWT validation)
    let protected_routes = Router::new()
        // Token operations
        .route("/revoke", post(revoke_token_handler))
        // Root key management
        .route("/keys", get(list_keys_handler))
        .route("/keys", post(create_key_handler))
        .route("/keys/:key_id", delete(delete_key_handler))
        // Client key management
        .route("/keys/clients", get(list_clients_handler))
        .route("/client-key", post(generate_client_key_handler))
        .route(
            "/keys/:key_id/clients/:client_id",
            delete(delete_client_handler),
        )
        // Permission management for both root and client keys
        .route(
            "/keys/:key_id/permissions",
            get(get_key_permissions_handler).put(update_key_permissions_handler),
        )
        // Protected system endpoints
        .route("/identity", get(identity_handler))
        .route("/metrics", get(metrics_handler))
        // Add authentication middleware to all protected routes
        .layer(from_fn(auth_middleware));

    // Create the base router with all routes
    let mut router = Router::new()
        .nest("/auth", public_routes)
        .nest("/admin", protected_routes)
        .layer(Extension(Arc::clone(&state)));

    // Add security layers from outermost to innermost

    // 1. Add CORS layer first (outermost)
    router = router.layer(cors_layer);

    // 2. Add security headers if enabled
    if config.security.headers.enabled {
        for header_layer in create_security_headers(&config.security.headers) {
            router = router.layer(header_layer);
        }
    }

    // 3. Add body size limiting (innermost)
    router = router.layer(create_body_limit_layer(config.security.max_body_size));

    // 4. Catch any handler/middleware panic and turn it into a 500 response
    //    instead of aborting the connection. This is the outermost layer so it
    //    guards every inner layer and handler against panic-induced DoS.
    router = router.layer(CatchPanicLayer::new());

    router
}

#[cfg(test)]
mod cors_tests {
    //! Guards the `Access-Control-Expose-Headers` contract for the standalone
    //! `mero-auth` service. A browser hides every response header from JS unless
    //! the server names it here — so without `x-auth-error`, a cross-origin
    //! client cannot see `token_expired` (breaking automatic refresh) nor
    //! `token_reuse` (leaving it unable to recognise a revoked token family).
    //! `mero-server` has the same guard after a prior production incident.

    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;
    use tower_http::cors::CorsLayer;

    use crate::config::CorsConfig;

    /// The CORS layer exactly as `create_router` builds it for an explicit
    /// allow-list of origins (the locked-down production shape).
    fn cors_layer_for(config: &CorsConfig) -> CorsLayer {
        let mut layer = CorsLayer::new();
        for origin in &config.allowed_origins {
            layer = layer.allow_origin(origin.parse::<axum::http::HeaderValue>().unwrap());
        }
        layer = layer.allow_methods(
            config
                .allowed_methods
                .iter()
                .filter_map(|m| m.parse().ok())
                .collect::<Vec<_>>(),
        );
        layer = layer.allow_headers(
            config
                .allowed_headers
                .iter()
                .filter_map(|h| h.parse::<axum::http::HeaderName>().ok())
                .collect::<Vec<_>>(),
        );
        layer.expose_headers([
            axum::http::HeaderName::from_static("x-auth-error"),
            axum::http::HeaderName::from_static("x-auth-user"),
            axum::http::HeaderName::from_static("x-auth-permissions"),
        ])
    }

    #[tokio::test]
    async fn explicit_origin_cors_exposes_x_auth_error() {
        let config = CorsConfig {
            allow_all_origins: false,
            allowed_origins: vec!["http://localhost:5173".to_string()],
            allowed_methods: vec!["GET".to_string(), "POST".to_string()],
            allowed_headers: vec!["authorization".to_string()],
            ..Default::default()
        };

        let app = Router::new()
            .route("/x", get(|| async { StatusCode::OK }))
            .layer(cors_layer_for(&config));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/x")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("router call must not fail");

        let exposed = resp
            .headers()
            .get(header::ACCESS_CONTROL_EXPOSE_HEADERS)
            .expect(
                "missing Access-Control-Expose-Headers — a cross-origin browser client \
                 cannot see x-auth-error, so it can neither auto-refresh on token_expired \
                 nor detect a revoked family on token_reuse",
            )
            .to_str()
            .expect("header must be ASCII")
            .to_ascii_lowercase();

        assert!(
            exposed.contains("x-auth-error"),
            "x-auth-error must be exposed to JS clients, got: {exposed}"
        );
    }
}
