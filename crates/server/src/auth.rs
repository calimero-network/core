#![cfg(feature = "bundled-auth")]

use axum::Router;
use eyre::Result;
use tracing::info;

use mero_auth::embedded::{build_app, default_config, EmbeddedAuthApp};

use crate::config::ServerConfig;

/// Wrapper around the embedded authentication application, keeping the router and shared state.
pub struct BundledAuth {
    app: EmbeddedAuthApp,
}

impl BundledAuth {
    #[must_use]
    pub fn into_router(self) -> Router {
        self.app.router
    }
}

/// Initialise the embedded authentication service according to the server configuration.
pub async fn initialise(_server_config: &ServerConfig) -> Result<BundledAuth> {
    // TODO(calimero): allow overriding the default configuration via `ServerConfig`.
    let auth_config = default_config();

    let app = build_app(auth_config).await?;

    info!("Bundled authentication endpoints enabled at /auth and /admin");

    Ok(BundledAuth { app })
}
