#![cfg(feature = "near_client")]

//! NEAR-specific implementations for context config mutations.
//!
//! This module provides NEAR blockchain-specific implementations of the `Method<Near>`
//! trait for context config mutation operations. It handles NEAR's JSON-based
//! encoding and decoding using `serde_json`.
//!
//! ## Key Features
//!
//! - **JSON Encoding**: Uses JSON for parameter encoding and response decoding
//! - **ED25519 Signing**: Implements ED25519 signature generation and verification
//! - **Type Safety**: Leverages serde for type-safe JSON interactions
//! - **Error Handling**: Converts NEAR-specific errors to generic `eyre::Result`
//!
//! ## Implementation Details
//!
//! The mutation request is encoded using JSON:
//! - ED25519 keys are used directly for NEAR compatibility
//! - Request data is encoded using JSON serialization
//! - Signatures are generated using ED25519
//! - Responses are decoded using JSON deserialization

use ed25519_dalek::{Signer, SigningKey};

use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::repr::ReprTransmute;
use crate::types::Signed;
use crate::Request;

impl<'a> Method<Near> for Mutate<'a> {
    const METHOD: &'static str = "mutate";

    type Returns = ();

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let request = Request::new(signer_sk.verifying_key().rt()?, self.kind, self.nonce);

        let signed = Signed::new(&request, |b| signer_sk.sign(b))?;

        let encoded = serde_json::to_vec(&signed)?;

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if !response.is_empty() {
            eyre::bail!("unexpected response {:?}", response);
        }

        Ok(())
    }
}
