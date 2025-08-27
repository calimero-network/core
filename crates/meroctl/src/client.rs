//! API client for meroctl
//!
//! This module provides type aliases and re-exports for the generic
//! Client from calimero-client, specialized for meroctl's needs.

use calimero_client::client::Client as GenericClient;

use crate::auth::CliAuthenticator;
use crate::storage::FileTokenStorage;

/// Type alias for Client specialized for meroctl
///
/// This uses the generic Client from calimero-client with
/// meroctl's concrete authenticator and storage implementations.
pub type Client = GenericClient<CliAuthenticator, FileTokenStorage>;

// Re-export response types and traits for convenience
pub use calimero_client::client::{ResolveResponse, ResolveResponseValue};
