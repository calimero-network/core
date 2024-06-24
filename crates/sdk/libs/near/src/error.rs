use thiserror::Error;

#[derive(Debug, Error)]
pub enum NearLibError {
    #[error(transparent)]
    JsonError(#[from] serde_json::Error),

    #[error("Failed to fetch: {0}")]
    FetchError(String),
}
