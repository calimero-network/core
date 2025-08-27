//! Error types for Calimero client operations
//!
//! This module defines the main error type for client operations.

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

    /// Internal errors
    #[error("Internal error: {message}")]
    Internal { message: String },
}

// Implement From for common error types
impl From<reqwest::Error> for ClientError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            ClientError::Network {
                message: "Request timeout".to_string(),
            }
        } else if err.is_connect() {
            ClientError::Network {
                message: format!("Connection failed: {}", err),
            }
        } else if err.is_status() {
            if let Some(status) = err.status() {
                ClientError::Network {
                    message: format!("HTTP {}: {}", status.as_u16(), err),
                }
            } else {
                ClientError::Network {
                    message: format!("HTTP error: {}", err),
                }
            }
        } else {
            ClientError::Network {
                message: format!("Network error: {}", err),
            }
        }
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(err: serde_json::Error) -> Self {
        ClientError::Internal {
            message: format!("Serialization error: {}", err),
        }
    }
}

impl From<std::io::Error> for ClientError {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => ClientError::Storage {
                message: "File not found".to_string(),
            },
            std::io::ErrorKind::PermissionDenied => ClientError::Storage {
                message: "Permission denied".to_string(),
            },
            _ => ClientError::Storage {
                message: format!("IO error: {}", err),
            },
        }
    }
}

impl From<url::ParseError> for ClientError {
    fn from(err: url::ParseError) -> Self {
        ClientError::Network {
            message: format!("Invalid URL: {}", err),
        }
    }
}
