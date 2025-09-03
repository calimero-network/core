#![cfg(feature = "ethereum_client")]

//! Ethereum-specific implementations for context config mutations.
//!
//! This module provides Ethereum blockchain-specific implementations of the `Method<Ethereum>`
//! trait for context config mutation operations. It handles Ethereum's Solidity ABI
//! encoding and decoding using the `alloy` and `alloy_sol_types` crates.
//!
//! ## Key Features
//!
//! - **ABI Encoding**: Uses Solidity ABI for parameter encoding and response decoding
//! - **ECDSA Signing**: Implements ECDSA signature generation and verification
//! - **Type Safety**: Leverages `alloy_sol_types` for type-safe Solidity interactions
//! - **Error Handling**: Converts Ethereum-specific errors to generic `eyre::Result`
//!
//! ## Implementation Details
//!
//! The mutation request is encoded using Solidity ABI encoding:
//! - ED25519 keys are derived to ECDSA keys for Ethereum compatibility
//! - Request data is encoded using ABI tuple encoding
//! - Signatures are generated using ECDSA with keccak256 message hashing
//! - Responses are decoded using dynamic ABI decoding with `SolValue`

use alloy::primitives::{keccak256, B256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::{Signature, SignerSync};
use alloy_sol_types::SolValue;
use ed25519_dalek::SigningKey;

use super::super::types::ethereum::{SolRequest, SolRequestKind, SolSignedRequest, ToSol};
use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::RequestKind;

impl<'a> Method<Ethereum> for Mutate<'a> {
    type Returns = ();
    // The method needs to be encoded as a tuple with arguments that it expects
    const METHOD: &'static str =
        "mutate(((bytes32,bytes32,uint64,uint8,bytes),bytes32,bytes32,uint8))";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let ed25519_key = SigningKey::from_bytes(&self.signing_key);
        let user_id_bytes = ed25519_key.verifying_key().to_bytes();
        let user_id = B256::from_slice(&user_id_bytes);

        let ecdsa_private_key_input =
            ["ECDSA_DERIVE".as_bytes(), &self.signing_key.as_slice()].concat();
        let ecdsa_private_key_bytes = keccak256(&ecdsa_private_key_input);
        let signer = PrivateKeySigner::from_bytes(&ecdsa_private_key_bytes)?;
        let address = signer.address();
        let ecdsa_public_key = address.into_word();

        let context_request = match &self.kind {
            RequestKind::Context(req) => req.to_sol(),
        };

        let encoded_request = context_request.abi_encode();

        let sol_request = SolRequest {
            signerId: ecdsa_public_key,
            userId: user_id,
            nonce: self.nonce,
            kind: SolRequestKind::Context,
            data: encoded_request.into(),
        };

        let request_message = sol_request.abi_encode();

        let message_hash = keccak256(&request_message);
        let signature: Signature = signer.sign_message_sync(&message_hash.as_slice())?;

        let r = B256::from(signature.r());
        let s = B256::from(signature.s());
        let v = if signature.recid().to_byte() == 0 {
            27
        } else {
            28
        };

        let signed_request = SolSignedRequest {
            payload: sol_request,
            r,
            s,
            v,
        };

        let encoded = signed_request.abi_encode();

        Ok(encoded)
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        Ok(())
    }
}
