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
//! 
//! ## Quick Start
//! 
//! ```rust
//! use calimero_client::{
//!     ClientAuthenticator, ClientStorage, ConnectionInfo, ClientError
//! };
//! 
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create your implementations of the traits
//!     let authenticator = MyAuthenticator::new();
//!     let storage = MyStorage::new();
//!     
//!     // Create a connection
//!     let connection = ConnectionInfo::new(
//!         "https://api.calimero.network".parse()?,
//!         None,
//!         Some("my-node".to_string()),
//!         authenticator,
//!         storage,
//!     );
//!     
//!     // Use the connection
//!     let response = connection.get("/health").await?;
//!     println!("Health: {:?}", response);
//!     
//!     Ok(())
//! }
//! ```

pub mod auth;
pub mod client;
pub mod connection;
pub mod errors;
pub mod storage;
pub mod traits;

// Re-export main types for easy access
pub use auth::CliAuthenticator;
pub use client::Client;
pub use connection::ConnectionInfo;
pub use errors::ClientError;
pub use storage::JwtToken;
pub use traits::{ClientAuthenticator, ClientStorage, ClientConfig};

// Re-export common types
pub use url::Url;
pub use eyre::Result;

/// Current version of the client library
pub const VERSION: &str = env!("CARGO_PKG_VERSION");