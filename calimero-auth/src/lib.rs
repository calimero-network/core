use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderMap, Request};

pub mod api;
pub mod auth;
pub mod config;
pub mod error;
pub mod providers;
pub mod server;
pub mod storage;

pub use auth::{
    AuthProvider, 
    AuthRequestVerifier, 
    AuthResponse,
    AuthVerifierFn,
    service::AuthService,
    middleware::AuthMiddleware,
};
pub use config::{AuthConfig, default_config, load_config};
pub use error::AuthError;
pub use server::start_server;
pub use storage::Storage; 