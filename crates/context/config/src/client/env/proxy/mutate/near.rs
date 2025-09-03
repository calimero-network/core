#![cfg(feature = "near_client")]

//! NEAR Protocol specific implementations for context proxy mutations.
//!
//! This module provides NEAR Protocol blockchain-specific implementations of the
//! `Method<Near>` trait for context proxy mutation operations. It handles NEAR's
//! JSON-based serialization format using the `serde_json` crate.
//!
//! ## Key Features
//!
//! - **JSON Serialization**: Uses JSON format for parameter encoding and response decoding
//! - **ED25519 Signing**: Implements ED25519 signature generation and verification
//! - **Simple Integration**: Leverages standard JSON for easy debugging and inspection
//! - **Error Handling**: Converts NEAR-specific errors to generic `eyre::Result`
//!
//! ## Implementation Details
//!
//! The mutation request is encoded using JSON serialization:
//! - Request data is signed using ED25519 with JSON message encoding
//! - Signatures are wrapped in a `Signed` type for proper serialization
//! - Responses are decoded using `serde_json::from_slice` for type safety
//! - Simple and efficient for NEAR's transaction architecture
//!
//! ## Usage
//!
//! These implementations are used automatically by the `ContextProxyMutate` client
//! when the underlying transport is configured for NEAR Protocol. No direct usage is required.

use ed25519_dalek::{Signer, SigningKey};

use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::types::Signed;
use crate::ProposalWithApprovals;

impl Method<Near> for Mutate {
    const METHOD: &'static str = "mutate";

    type Returns = Option<ProposalWithApprovals>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let signed = Signed::new(&self.raw_request, |b| signer_sk.sign(b))?;

        let encoded = serde_json::to_vec(&signed)?;

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}
