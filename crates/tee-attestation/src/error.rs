//! Error types for TEE attestation operations.

use std::fmt;

/// Error type for TEE attestation operations.
#[derive(Debug)]
pub enum AttestationError {
    /// TEE attestation is not supported on this platform.
    NotSupported,

    /// Failed to generate TDX quote.
    QuoteGenerationFailed(String),

    /// Failed to parse TDX quote.
    QuoteParsingFailed(String),

    /// Failed to convert quote to serializable format.
    QuoteConversionFailed(String),

    /// Failed to verify quote signature.
    QuoteVerificationFailed(String),

    /// Failed to fetch collateral from Intel PCS.
    CollateralFetchFailed(String),

    /// Invalid nonce format or length.
    InvalidNonce(String),

    /// Invalid application hash format or length.
    InvalidApplicationHash(String),

    /// Nonce verification failed.
    NonceMismatch { expected: String, actual: String },

    /// Application hash verification failed.
    ApplicationHashMismatch { expected: String, actual: String },

    /// Failed to get TEE info.
    InfoRetrievalFailed(String),

    /// System time error.
    SystemTimeError(String),
}

impl std::error::Error for AttestationError {}

impl fmt::Display for AttestationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSupported => {
                write!(
                    f,
                    "TEE attestation is not supported on this platform (requires Linux with TDX)"
                )
            }
            Self::QuoteGenerationFailed(msg) => write!(f, "Failed to generate TDX quote: {}", msg),
            Self::QuoteParsingFailed(msg) => write!(f, "Failed to parse TDX quote: {}", msg),
            Self::QuoteConversionFailed(msg) => {
                write!(f, "Failed to convert quote to serializable format: {}", msg)
            }
            Self::QuoteVerificationFailed(msg) => {
                write!(f, "Failed to verify quote signature: {}", msg)
            }
            Self::CollateralFetchFailed(msg) => {
                write!(f, "Failed to fetch collateral from Intel PCS: {}", msg)
            }
            Self::InvalidNonce(msg) => write!(f, "Invalid nonce: {}", msg),
            Self::InvalidApplicationHash(msg) => write!(f, "Invalid application hash: {}", msg),
            Self::NonceMismatch { expected, actual } => {
                write!(f, "Nonce mismatch: expected {}, got {}", expected, actual)
            }
            Self::ApplicationHashMismatch { expected, actual } => {
                write!(
                    f,
                    "Application hash mismatch: expected {}, got {}",
                    expected, actual
                )
            }
            Self::InfoRetrievalFailed(msg) => write!(f, "Failed to get TEE info: {}", msg),
            Self::SystemTimeError(msg) => write!(f, "System time error: {}", msg),
        }
    }
}
