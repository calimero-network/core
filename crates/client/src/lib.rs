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
pub use storage::{get_session_cache, JwtToken};
pub use traits::{ClientAuthenticator, ClientConfig, ClientStorage};
// Re-export common types
pub use url::Url;

/// Current version of the client library
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
use tokio_test as _;
