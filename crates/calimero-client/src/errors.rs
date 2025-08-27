//! Error types for Calimero client operations
//! 
//! This module defines all the error types that can occur during client
//! operations, including authentication, network, and storage errors.

use serde::Serialize;
use thiserror::Error;

/// Main error type for Calimero client operations
#[derive(Debug, Error)]
pub enum ClientError {
    /// Authentication-related errors
    #[error("Authentication error: {0}")]
    Authentication(#[from] AuthError),
    
    /// Network and HTTP-related errors
    #[error("Network error: {0}")]
    Network(#[from] NetworkError),
    
    /// Storage-related errors
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    
    /// Configuration-related errors
    #[error("Configuration error: {0}")]
    Configuration(#[from] ConfigError),
    
    /// Validation errors
    #[error("Validation error: {0}")]
    Validation(#[from] ValidationError),
    
    /// Internal errors
    #[error("Internal error: {0}")]
    Internal(#[from] InternalError),
}

/// Authentication-specific errors
#[derive(Debug, Error)]
pub enum AuthError {
    /// Invalid credentials provided
    #[error("Invalid credentials: {message}")]
    InvalidCredentials { message: String },
    
    /// Authentication token expired
    #[error("Authentication token expired")]
    TokenExpired,
    
    /// Authentication token invalid
    #[error("Authentication token invalid: {reason}")]
    TokenInvalid { reason: String },
    
    /// Authentication flow failed
    #[error("Authentication flow failed: {reason}")]
    FlowFailed { reason: String },
    
    /// Authentication not supported for this endpoint
    #[error("Authentication not supported for endpoint: {endpoint}")]
    NotSupported { endpoint: String },
    
    /// Rate limiting during authentication
    #[error("Rate limited during authentication: retry after {retry_after}")]
    RateLimited { retry_after: String },
}

/// Network and HTTP-related errors
#[derive(Debug, Error)]
pub enum NetworkError {
    /// HTTP request failed
    #[error("HTTP {status_code}: {message}")]
    HttpError {
        status_code: u16,
        message: String,
    },
    
    /// Network connection failed
    #[error("Network connection failed: {reason}")]
    ConnectionFailed { reason: String },
    
    /// Request timeout
    #[error("Request timeout after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
    
    /// SSL/TLS error
    #[error("SSL/TLS error: {reason}")]
    TlsError { reason: String },
    
    /// DNS resolution failed
    #[error("DNS resolution failed: {hostname}")]
    DnsError { hostname: String },
    
    /// Rate limiting
    #[error("Rate limited: retry after {retry_after}")]
    RateLimited { retry_after: String },
    
    /// Server error
    #[error("Server error: {message}")]
    ServerError { message: String },
}

/// Storage-related errors
#[derive(Debug, Error)]
pub enum StorageError {
    /// File not found
    #[error("File not found: {path}")]
    FileNotFound { path: String },
    
    /// Permission denied
    #[error("Permission denied: {path}")]
    PermissionDenied { path: String },
    
    /// Disk full
    #[error("Disk full: {path}")]
    DiskFull { path: String },
    
    /// Corrupted data
    #[error("Corrupted data: {reason}")]
    CorruptedData { reason: String },
    
    /// Encryption error
    #[error("Encryption error: {reason}")]
    EncryptionError { reason: String },
    
    /// Database error
    #[error("Database error: {reason}")]
    DatabaseError { reason: String },
}

/// Configuration-related errors
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Invalid configuration file
    #[error("Invalid configuration file: {path}")]
    InvalidFile { path: String },
    
    /// Missing required configuration
    #[error("Missing required configuration: {key}")]
    MissingConfig { key: String },
    
    /// Invalid configuration value
    #[error("Invalid configuration value for {key}: {value}")]
    InvalidValue { key: String, value: String },
    
    /// Configuration file not found
    #[error("Configuration file not found: {path}")]
    FileNotFound { path: String },
    
    /// Configuration file not readable
    #[error("Configuration file not readable: {path}")]
    FileNotReadable { path: String },
}

/// Validation errors
#[derive(Debug, Error)]
pub enum ValidationError {
    /// Invalid URL
    #[error("Invalid URL: {url}")]
    InvalidUrl { url: String },
    
    /// Invalid node name
    #[error("Invalid node name: {name}")]
    InvalidNodeName { name: String },
    
    /// Invalid token format
    #[error("Invalid token format: {reason}")]
    InvalidToken { reason: String },
    
    /// Missing required field
    #[error("Missing required field: {field}")]
    MissingField { field: String },
    
    /// Field value out of range
    #[error("Field {field} value {value} out of range [{min}, {max}]")]
    ValueOutOfRange { field: String, value: String, min: String, max: String },
}

/// Internal errors (should not occur in normal operation)
#[derive(Debug, Error)]
pub enum InternalError {
    /// Unexpected internal state
    #[error("Unexpected internal state: {message}")]
    UnexpectedState { message: String },
    
    /// Serialization error
    #[error("Serialization error: {reason}")]
    SerializationError { reason: String },
    
    /// Deserialization error
    #[error("Deserialization error: {reason}")]
    DeserializationError { reason: String },
    
    /// Type conversion error
    #[error("Type conversion error: {reason}")]
    ConversionError { reason: String },
}

// Implement From for common error types
impl From<reqwest::Error> for ClientError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            ClientError::Network(NetworkError::Timeout { 
                timeout_secs: 30 // Default timeout
            })
        } else if err.is_connect() {
            ClientError::Network(NetworkError::ConnectionFailed { 
                reason: err.to_string() 
            })
        } else if err.is_status() {
            if let Some(status) = err.status() {
                ClientError::Network(NetworkError::HttpError {
                    status_code: status.as_u16(),
                    message: err.to_string(),
                })
            } else {
                ClientError::Network(NetworkError::HttpError {
                    status_code: 0,
                    message: err.to_string(),
                })
            }
        } else {
            ClientError::Network(NetworkError::ConnectionFailed { 
                reason: err.to_string() 
            })
        }
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(err: serde_json::Error) -> Self {
        ClientError::Internal(InternalError::DeserializationError {
            reason: err.to_string(),
        })
    }
}

impl From<std::io::Error> for ClientError {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => {
                ClientError::Storage(StorageError::FileNotFound {
                    path: "unknown".to_string(),
                })
            }
            std::io::ErrorKind::PermissionDenied => {
                ClientError::Storage(StorageError::PermissionDenied {
                    path: "unknown".to_string(),
                })
            }
            _ => ClientError::Storage(StorageError::FileNotFound {
                path: err.to_string(),
            }),
        }
    }
}

impl From<url::ParseError> for ClientError {
    fn from(err: url::ParseError) -> Self {
        ClientError::Validation(ValidationError::InvalidUrl {
            url: err.to_string(),
        })
    }
}



// Implement Serialize for JSON responses
impl Serialize for ClientError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        
        let mut state = serializer.serialize_struct("ClientError", 3)?;
        state.serialize_field("type", &self.error_type())?;
        state.serialize_field("message", &self.to_string())?;
        state.serialize_field("details", &self.error_details())?;
        state.end()
    }
}

impl ClientError {
    /// Get the error type as a string
    pub fn error_type(&self) -> &'static str {
        match self {
            ClientError::Authentication(_) => "authentication",
            ClientError::Network(_) => "network",
            ClientError::Storage(_) => "storage",
            ClientError::Configuration(_) => "configuration",
            ClientError::Validation(_) => "validation",
            ClientError::Internal(_) => "internal",
        }
    }
    
    /// Get additional error details
    pub fn error_details(&self) -> serde_json::Value {
        match self {
            ClientError::Network(NetworkError::HttpError { status_code, message }) => {
                serde_json::json!({
                    "status_code": status_code,
                    "message": message
                })
            }
            ClientError::Authentication(AuthError::TokenExpired) => {
                serde_json::json!({
                    "suggestion": "Please re-authenticate"
                })
            }
            _ => serde_json::Value::Null,
        }
    }
    
    /// Check if this is a retryable error
    pub fn is_retryable(&self) -> bool {
        matches!(self, 
            ClientError::Network(NetworkError::Timeout { .. }) |
            ClientError::Network(NetworkError::ConnectionFailed { .. }) |
            ClientError::Network(NetworkError::RateLimited { .. })
        )
    }
    
    /// Check if this is an authentication error
    pub fn is_auth_error(&self) -> bool {
        matches!(self, ClientError::Authentication(_))
    }
}
