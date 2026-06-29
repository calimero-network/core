//! TEE attestation generation and verification for Calimero.
//!
//! This crate provides platform-agnostic interfaces for:
//! - Generating TDX attestation quotes (Linux with TDX)
//! - Mock attestation generation (non-Linux platforms, for development)
//! - Verifying TDX attestation quotes (cross-platform)
//! - Retrieving TEE host information
//!
//! # Example
//!
//! ```ignore
//! use calimero_tee_attestation::{generate_attestation, verify_attestation, build_report_data};
//!
//! // Generate an attestation, binding it to an application/identity hash
//! let nonce = [0u8; 32];
//! let app_hash = [1u8; 32];
//! let report_data = build_report_data(&nonce, Some(&app_hash));
//! let result = generate_attestation(report_data)?;
//!
//! // On non-Linux, result.is_mock will be true
//! if result.is_mock {
//!     println!("Generated mock attestation for development");
//! }
//!
//! // Verify an attestation (use verify_mock_attestation for mock quotes).
//! // The expected application hash is mandatory: a quote that does not bind it
//! // can never be considered valid.
//! let verification = if result.is_mock {
//!     verify_mock_attestation(&result.quote_bytes, &nonce, &app_hash)?
//! } else {
//!     verify_attestation(&result.quote_bytes, &nonce, &app_hash).await?
//! };
//! assert!(verification.crypto_valid());
//! ```
//!
//! # Platform Behavior
//!
//! - **Linux with TDX**: Generates real TDX attestation quotes that can be
//!   cryptographically verified.
//! - **Non-Linux platforms**: Generates mock attestations (`is_mock = true`)
//!   for development and testing. These are NOT cryptographically valid.
//!
//! **Warning**: Mock attestations bypass all TEE security guarantees and should
//! never be trusted in production environments.

mod error;
mod generate;
mod info;
mod verify;

pub use error::AttestationError;
pub use generate::{
    build_report_data, generate_attestation, generate_mock_attestation, is_mock_quote,
    AttestationResult,
};
pub use info::{get_tee_info, TeeInfo};
pub use verify::{verify_attestation, verify_mock_attestation, VerificationResult};
