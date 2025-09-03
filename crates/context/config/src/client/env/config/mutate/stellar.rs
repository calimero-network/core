#![cfg(feature = "stellar_client")]

//! Stellar-specific implementations for context config mutations.
//!
//! This module provides Stellar blockchain-specific implementations of the `Method<Stellar>`
//! trait for context config mutation operations. It handles Stellar's XDR-based
//! encoding and decoding using `soroban_sdk`.
//!
//! ## Key Features
//!
//! - **XDR Encoding**: Uses XDR for parameter encoding and response decoding
//! - **ED25519 Signing**: Implements ED25519 signature generation and verification
//! - **Type Safety**: Leverages Soroban SDK for type-safe interactions
//! - **Error Handling**: Converts Stellar-specific errors to generic `eyre::Result`
//!
//! ## Implementation Details
//!
//! The mutation request is encoded using XDR:
//! - ED25519 keys are used directly for Stellar compatibility
//! - Request data is encoded using XDR serialization
//! - Signatures are generated using ED25519
//! - Responses are decoded using XDR deserialization

use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{BytesN, Env};

use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::stellar::Stellar;
use crate::repr::ReprTransmute;
use crate::stellar::stellar_types::{
    FromWithEnv, StellarRequest, StellarRequestKind, StellarSignedRequest,
    StellarSignedRequestPayload,
};

impl<'a> Method<Stellar> for Mutate<'a> {
    type Returns = ();
    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let signer_id: [u8; 32] = signer_sk.verifying_key().rt()?;
        let signer_id = BytesN::from_array(&env, &signer_id);

        let request = StellarRequest::new(
            signer_id,
            StellarRequestKind::from_with_env(self.kind, &env),
            self.nonce,
        );

        let signed_request_payload = StellarSignedRequestPayload::Context(request);

        let signed_request =
            StellarSignedRequest::new(&env, signed_request_payload, |b| Ok(signer_sk.sign(b)))
                .map_err(|e| eyre::eyre!("Failed to sign request: {:?}", e))?;

        let bytes: Vec<u8> = signed_request.to_xdr(&env).into_iter().collect();

        Ok(bytes)
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        Ok(())
    }
}
