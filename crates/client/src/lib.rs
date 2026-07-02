//! Calimero Client Library
//!
//! A comprehensive, abstract client library for interacting with Calimero APIs.
//! This library provides trait-based abstractions for authentication, storage,
//! and API communication, making it easy to implement different client types
//! (CLI, GUI, headless, etc.) while sharing common functionality.
//!
//! ## Features
//!
//! - **Abstract Interfaces**: Trait-based design for maximum flexibility
//! - **Authentication**: Support for various authentication methods
//! - **Token Storage**: Abstract token management with multiple backends
//! - **HTTP Client**: Robust HTTP client with retry and error handling
//! - **Async Support**: Full async/await support throughout
//! - **Python Bindings**: Optional Python bindings via PyO3

pub mod auth;
pub mod client;
pub mod connection;
pub mod errors;
pub mod storage;
pub mod traits;

// Re-export main types for easy access
pub use auth::{CliAuthenticator, MeroctlOutputHandler};
pub use client::{Client, ResolveResponse, ResolveResponseValue};
pub use connection::{AuthMode, ConnectionInfo};
pub use errors::ClientError;
pub use eyre::Result;
pub use storage::JwtToken;
pub use traits::{ClientAuthenticator, ClientConfig, ClientStorage};
// Re-export common types
pub use url::Url;

/// Current version of the client library
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod test_support {
    use base64::Engine as _;

    /// Build a minimal JWT (`header.payload.sig`) whose payload carries the
    /// given `exp` (seconds since the Unix epoch). Only the `exp` claim is read
    /// by the client, so the header/signature segments are placeholders. Shared
    /// by the `storage` and `tests` modules to avoid drift.
    pub(crate) fn jwt_with_exp(exp_unix: i64) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!("{{\"exp\":{exp_unix}}}"));
        format!("aGVhZGVy.{payload}.c2ln")
    }
}

#[cfg(test)]
use tokio_test as _;

#[cfg(test)]
mod tests;
