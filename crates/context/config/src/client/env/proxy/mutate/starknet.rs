#![cfg(feature = "starknet_client")]

//! Starknet specific implementations for context proxy mutations.
//!
//! This module provides Starknet blockchain-specific implementations of the
//! `Method<Starknet>` trait for context proxy mutation operations. It handles
//! Starknet's Cairo serialization format using the `starknet_core` and `starknet_crypto` crates.
//!
//! ## Key Features
//!
//! - **Cairo Serialization**: Uses Cairo's native serialization for parameter encoding and response decoding
//! - **ECDSA Signing**: Implements ECDSA signature generation using Starknet's signing key
//! - **Poseidon Hashing**: Uses Poseidon hash function for message hashing
//! - **Error Handling**: Converts Starknet-specific errors to generic `eyre::Result`
//!
//! ## Implementation Details
//!
//! The mutation request is encoded using Cairo serialization:
//! - ED25519 keys are derived to ECDSA keys for Starknet compatibility
//! - Request data is serialized and hashed using Poseidon hash function
//! - Signatures are generated using ECDSA with Starknet's signing key
//! - Responses are decoded by parsing Cairo-encoded field elements
//! - 32-byte chunks are processed for proper field element alignment
//!
//! ## Usage
//!
//! These implementations are used automatically by the `ContextProxyMutate` client
//! when the underlying transport is configured for Starknet. No direct usage is required.

use eyre::WrapErr;
use starknet::core::codec::Encode;
use starknet::signers::SigningKey as StarknetSigningKey;
use starknet_crypto::{poseidon_hash_many, Felt};

use super::super::types::starknet::{StarknetProxyMutateRequest, StarknetSignedRequest};
use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::starknet::Starknet;
use crate::repr::ReprTransmute;
use crate::{ProposalWithApprovals, Repr};

impl Method<Starknet> for Mutate {
    type Returns = Option<ProposalWithApprovals>;
    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Derive ECDSA key for signing
        let secret_scalar = Felt::from_bytes_be(&self.signing_key);
        let signing_key = StarknetSigningKey::from_secret_scalar(secret_scalar);
        let verifying_key = signing_key.verifying_key().scalar();
        let verifying_key_bytes = verifying_key.to_bytes_be();

        // Create signer_id from ECDSA verifying key for signature verification
        let signer_id = verifying_key_bytes.rt().wrap_err("Infallible conversion")?;

        // Create request with signer_id (no Repr)
        let request = StarknetProxyMutateRequest::from((signer_id, self.raw_request));

        // Serialize -> Hash -> Sign with ECDSA
        let mut serialized_request = vec![];
        request.encode(&mut serialized_request)?;
        let hash = poseidon_hash_many(&serialized_request);
        let signature = signing_key.sign(&hash)?;

        let signed_request = StarknetSignedRequest {
            payload: serialized_request,
            signature_r: signature.r,
            signature_s: signature.s,
        };

        let mut signed_request_serialized = vec![];
        signed_request.encode(&mut signed_request_serialized)?;

        let bytes: Vec<u8> = signed_request_serialized
            .iter()
            .flat_map(|felt| felt.to_bytes_be())
            .collect();

        Ok(bytes)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(None);
        }

        // Skip first 32 bytes (array length)
        let response = &response[32..];

        // Get proposal_id from the next 64 bytes (32 for high, 32 for low)
        let mut proposal_bytes = [0u8; 32];
        proposal_bytes[..16].copy_from_slice(&response[16..32]); // Last 16 bytes of high
        proposal_bytes[16..].copy_from_slice(&response[48..64]); // Last 16 bytes of low
        let proposal_id = Repr::new(proposal_bytes.rt()?);

        // Get num_approvals from the last 32 bytes
        let num_approvals = u32::from_be_bytes(response[64..][28..32].try_into()?) as usize;

        Ok(Some(ProposalWithApprovals {
            proposal_id,
            num_approvals,
        }))
    }
}
