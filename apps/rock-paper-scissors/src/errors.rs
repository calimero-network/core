use std::fmt;

use calimero_sdk::serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum Error {
    ConversionError,
    ResetError,
    ByteSizeError,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ConversionError => write!(f, "Conversion error occurred"),
            Error::ResetError => write!(f, "Reset error occurred"),
            Error::ByteSizeError => write!(f, "Byte size error occurred"),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum JoinError {
    GameFull,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum CommitError {
    OtherNotJoined,
    PlayerNotFound,
    InvalidSignature,
    AlreadyCommitted,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum RevealError {
    PlayerNotFound,
    InvalidNonce,
    NotCommitted,
    NotRevealed,
}
