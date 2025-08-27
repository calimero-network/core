use serde::Serialize;
use thiserror::Error;

/// Client-specific error type for HTTP responses
#[derive(Debug, Serialize, Error)]
#[error("{status_code}: {message}")]
pub struct ClientError {
    pub status_code: u16,
    pub message: String,
}
