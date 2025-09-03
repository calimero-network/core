//! Unified error handling for the context crate

use thiserror::Error;

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ContextError {
    #[error("context not found")]
    ContextNotFound,
    
    #[error("application not found")]
    ApplicationNotFound,
    
    #[error("invalid signature")]
    InvalidSignature,
    
    #[error("permission denied")]
    PermissionDenied,
    
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    
    #[cfg(feature = "client")]
    #[error("network error: {0}")]
    NetworkError(#[from] reqwest::Error),
    
    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
    
    // Map borsh errors into eyre for now to avoid version coupling
    #[error("borsh error: {0}")]
    BorshError(String),
    
    #[error("crypto error: {0}")]
    CryptoError(String),
    
    #[error("transport error: {0}")]
    TransportError(String),
    
    #[error("internal error: {0}")]
    InternalError(#[from] eyre::Error),
}

pub type ContextResult<T> = Result<T, ContextError>;
