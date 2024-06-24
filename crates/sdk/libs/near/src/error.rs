use thiserror::Error;

use crate::jsonrpc::RpcError;

#[derive(Debug, Error)]
pub enum Error<R> {
    #[error(transparent)]
    JsonError(#[from] serde_json::Error),

    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error("Failed to fetch: {0}")]
    FetchError(String),

    #[error(transparent)]
    ServerError(#[from] RpcError<R>),
}
