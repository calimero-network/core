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
//! // Generate an attestation
//! let nonce = [0u8; 32];
//! let report_data = build_report_data(&nonce, None);
//! let result = generate_attestation(report_data)?;
//!
//! // On non-Linux, result.is_mock will be true
//! if result.is_mock {
//!     println!("Generated mock attestation for development");
//! }
//!
//! // Verify an attestation (use verify_mock_attestation for mock quotes)
//! let verification = if result.is_mock {
//!     verify_mock_attestation(&result.quote_bytes, &nonce, None)?
//! } else {
//!     verify_attestation(&result.quote_bytes, &nonce, None).await?
//! };
//! assert!(verification.is_valid());
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
pub use generate::{build_report_data, generate_attestation, is_mock_quote, AttestationResult};
pub use info::{get_tee_info, TeeInfo};
pub use verify::{verify_attestation, verify_mock_attestation, VerificationResult};
