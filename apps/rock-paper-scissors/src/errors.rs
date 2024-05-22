use calimero_sdk::serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum JoinError {
    GameFull,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum CommitError {
    NotReady,
    AlreadyCommitted,
    InvalidSignature,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum RevealError {
    NotReady,
    NotCommitted,
    InvalidNonce,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum ResetError {
    NotReady,
    InvalidSignature,
}
