use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use eyre::Result;
use tracing::info;

use crate::api::routes::create_router;
use crate::auth::token::TokenManager;
use crate::config::{
    AuthConfig, ContentSecurityPolicyConfig, DevelopmentConfig, JwtConfig, NearWalletConfig,
    SecurityConfig, SecurityHeadersConfig, StorageConfig, UserPasswordConfig,
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

    let secret_manager = Arc::new(SecretManager::new(Arc::clone(&storage)));
    secret_manager.initialize().await?;

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

    let state = Arc::new(AppState {
        auth_service: auth_service.clone(),
        storage,
        key_manager,
        token_generator: token_manager,
        config: config.clone(),
        metrics,
    });

    let router = create_router(Arc::clone(&state), &config);

    Ok(EmbeddedAuthApp { router, state })
}

/// Default configuration used when no explicit configuration is supplied.
#[must_use]
pub fn default_config() -> AuthConfig {
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
                    connect_src: vec![
                        "'self'".to_string(),
                        "http://localhost:*".to_string(),
                        "http://host.docker.internal:*".to_string(),
                        "http://*.nip.io:*".to_string(),
                        "https://*.nip.io:*".to_string(),
                    ],
                },
            },
        },
        providers,
        near: NearWalletConfig::default(),
        user_password: UserPasswordConfig::default(),
        development: DevelopmentConfig::default(),
    }
}
