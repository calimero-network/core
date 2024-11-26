use std::fmt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("Unauthorized access")]
    Unauthorized,
    
    #[error("Context not found")]
    ContextNotFound,
    
    #[error("Context already exists")]
    ContextExists,
    
    #[error("Request expired")]
    RequestExpired,
    
    #[error("Invalid signature")]
    InvalidSignature,
    
    #[error("Proxy code not set")]
    ProxyCodeNotSet,
    
    #[error("Proxy update failed: {0}")]
    ProxyUpdateFailed(String),
    
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<ContractError> for String {
    fn from(error: ContractError) -> Self {
        error.to_string()
    }
}