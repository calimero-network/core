use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use eyre::Result;
use tracing::info;

use crate::api::routes::create_router;
use crate::auth::token::TokenManager;
use crate::config::{
    AuthConfig, ContentSecurityPolicyConfig, DevelopmentConfig, JwtConfig, SecurityConfig,
    SecurityHeadersConfig, StorageConfig, UserPasswordConfig,
};
use crate::providers;
use crate::secrets::SecretManager;
use crate::server::AppState;
use crate::storage::{create_storage, KeyManager, Storage};
use crate::utils::AuthMetrics;
use crate::AuthService;

/// Fully-initialised authentication application that can be embedded into another service.
pub struct EmbeddedAuthApp {
    /// Axum router exposing both public and admin authentication routes.
    pub router: Router,
    /// Shared application state used by the authentication handlers.
    pub state: Arc<AppState>,
}

impl EmbeddedAuthApp {
    /// Convenience accessor for the underlying authentication service.
    #[must_use]
    pub fn auth_service(&self) -> AuthService {
        self.state.auth_service.clone()
    }

    /// Convenience accessor for the storage backend.
    #[must_use]
    pub fn storage(&self) -> Arc<dyn Storage> {
        Arc::clone(&self.state.storage)
    }

    /// Convenience accessor for the effective configuration.
    #[must_use]
    pub fn config(&self) -> AuthConfig {
        self.state.config.clone()
    }
}

/// Build an embedded authentication application from configuration.
///
/// This mirrors the standalone initialisation performed in [`crate::main`], but packages the result
/// so the router can be mounted inside another Axum application.
pub async fn build_app(config: AuthConfig) -> Result<EmbeddedAuthApp> {
    let storage = create_storage(&config.storage).await?;

    let secret_manager = Arc::new(SecretManager::with_storage_config(
        Arc::clone(&storage),
        &config.storage,
    ));
    secret_manager.initialize().await?;

    // Spawn the JWT signing-secret rotation task (finding #4). Safe to enable now
    // that verification accepts an unexpired backup secret (PR1), so a rotation no
    // longer mass-invalidates outstanding tokens.
    Arc::clone(&secret_manager).start_rotation_task().await;

    let token_manager = TokenManager::new(
        config.jwt.clone(),
        Arc::clone(&storage),
        Arc::clone(&secret_manager),
    );

    let providers =
        providers::create_providers(Arc::clone(&storage), &config, token_manager.clone())?;

    info!("Initialized {} authentication providers", providers.len());
    for provider in &providers {
        info!("  - {} ({})", provider.name(), provider.description());
    }

    let auth_service = AuthService::new(providers, token_manager.clone());

    let metrics = AuthMetrics::new();
    let key_manager = KeyManager::new(Arc::clone(&storage));

    // Surface the first-login setup code while the node is still
    // un-bootstrapped (no root keys yet), then never again. The code is
    // already at rest in the 0600 config.toml next to the node's private
    // key, so logging it here exposes nothing the node's owner can't read —
    // and without this, a fresh node's first login fails with an opaque 401
    // (core#3221 deliberately makes the rejection indistinguishable from a
    // wrong password).
    if config
        .providers
        .get("user_password")
        .copied()
        .unwrap_or(false)
    {
        match key_manager
            .list_keys(crate::storage::models::KeyType::Root)
            .await
        {
            Ok(keys) if keys.is_empty() => {
                if let Some(secret) = config.user_password.effective_bootstrap_secret() {
                    info!("==============================================================");
                    info!("No account exists on this node yet.");
                    info!("First-login setup code: {secret}");
                    info!("Log in with your chosen username/password plus this code to");
                    info!("create the admin account. (Also stored in config.toml.)");
                    info!("==============================================================");
                } else {
                    info!(
                        "No account exists on this node yet and no bootstrap secret is \
                         configured — first login will be rejected. Set \
                         MERO_AUTH_BOOTSTRAP_SECRET or [user_password] bootstrap_secret \
                         to enable first-login setup."
                    );
                }
            }
            Ok(_) => {}
            Err(err) => {
                info!("Could not determine bootstrap state: {err}");
            }
        }
    }

    let state = Arc::new(AppState {
        auth_service: auth_service.clone(),
        storage,
        key_manager,
        token_generator: token_manager,
        config: config.clone(),
        metrics,
        login_rate_limiter: Arc::new(crate::auth::rate_limit::LoginRateLimiter::default()),
    });

    let router = create_router(Arc::clone(&state), &config);

    Ok(EmbeddedAuthApp { router, state })
}

/// Default configuration used when no explicit configuration is supplied.
#[must_use]
pub fn default_config() -> AuthConfig {
    let mut providers = HashMap::new();
    providers.insert("user_password".to_string(), true);

    AuthConfig {
        listen_addr: "127.0.0.1:3001".parse().unwrap(),
        jwt: JwtConfig {
            issuer: "calimero-auth".to_string(),
            access_token_expiry: 3600,
            refresh_token_expiry: 2592000,
            // Opt-in (finding #7): unset keeps legacy header-derived node-host
            // validation. Operators set the node's public host to enforce
            // node-binding against trusted config instead of request headers.
            node_host: None,
        },
        storage: StorageConfig::RocksDB {
            path: "auth".into(),
        },
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
                    connect_src: vec![
                        "'self'".to_string(),
                        "http://localhost:*".to_string(),
                        "http://host.docker.internal:*".to_string(),
                        "http://*.nip.io:*".to_string(),
                        "https://*.nip.io:*".to_string(),
                        "https:".to_string(), // Allow all HTTPS connections for configurable registries
                        "http:".to_string(),  // Allow HTTP for local development registries
                    ],
                },
            },
        },
        providers,
        user_password: UserPasswordConfig::default(),
        development: DevelopmentConfig::default(),
    }
}
