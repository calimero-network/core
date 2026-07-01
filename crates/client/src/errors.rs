//! Error types for Calimero client operations
//!
//! This module defines the main error type for client operations.

// External crates
use serde::Serialize;
use thiserror::Error;

/// Main error type for Calimero client operations
#[derive(Debug, Error, Serialize)]
pub enum ClientError {
    /// Network and HTTP-related errors
    #[error("Network error: {message}")]
    Network { message: String },

    /// Authentication-related errors
    #[error("Authentication error: {message}")]
    Authentication { message: String },

    /// Storage-related errors
    #[error("Storage error: {message}")]
    Storage { message: String },

    /// A non-success HTTP status returned by the node's API.
    ///
    /// Carries the numeric `status` so callers can classify the failure
    /// (e.g. 404 → not-found) by matching this variant instead of parsing the
    /// rendered `message` string. `message` is already status-prefixed
    /// (`"HTTP {code}[: detail]"`) so the `Display` output is unchanged.
    #[error("{message}")]
    Http { status: u16, message: String },

    /// Internal errors
    #[error("Internal error: {message}")]
    Internal { message: String },
}

impl ClientError {
    /// Whether this error represents an HTTP 404 (resource not found).
    #[must_use]
    pub const fn is_not_found(&self) -> bool {
        matches!(self, ClientError::Http { status: 404, .. })
    }
}

// Implement From for common error types
impl From<reqwest::Error> for ClientError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            ClientError::Network {
                message: "Request timeout".to_owned(),
            }
        } else if err.is_connect() {
            ClientError::Network {
                message: format!("Connection failed: {err}"),
            }
        } else if err.is_status() {
            if let Some(status) = err.status() {
                ClientError::Network {
                    message: format!("HTTP {}: {}", status.as_u16(), err),
                }
            } else {
                ClientError::Network {
                    message: format!("HTTP error: {err}"),
                }
            }
        } else {
            ClientError::Network {
                message: format!("Network error: {err}"),
            }
        }
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(err: serde_json::Error) -> Self {
        ClientError::Internal {
            message: format!("Serialization error: {err}"),
        }
    }
}

impl From<std::io::Error> for ClientError {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => ClientError::Storage {
                message: "File not found".to_owned(),
            },
            std::io::ErrorKind::PermissionDenied => ClientError::Storage {
                message: "Permission denied".to_owned(),
            },
            _ => ClientError::Storage {
                message: format!("IO error: {err}"),
            },
        }
    }
}

impl From<url::ParseError> for ClientError {
    fn from(err: url::ParseError) -> Self {
        ClientError::Network {
            message: format!("Invalid URL: {err}"),
        }
    }
}
