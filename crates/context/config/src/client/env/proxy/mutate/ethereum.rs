#![cfg(feature = "ethereum_client")]

//! Ethereum-specific implementations for context proxy mutations.
//!
//! This module provides Ethereum blockchain-specific implementations of the `Method<Ethereum>`
//! trait for context proxy mutation operations. It handles Ethereum's Solidity ABI
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
//!
//! ## Usage
//!
//! These implementations are used automatically by the `ContextProxyMutate` client
//! when the underlying transport is configured for Ethereum. No direct usage is required.

use alloy::primitives::{keccak256, B256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::{Signature, SignerSync};
use alloy_sol_types::SolValue;
use ed25519_dalek::SigningKey;
use eyre::WrapErr;

use super::super::ethereum::{SolProposal, SolProposalApprovalWithSigner};
use super::super::types::ethereum::{SolRequest, SolRequestKind, SolSignedRequest};
use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::repr::ReprTransmute;
use crate::{ProposalWithApprovals, ProxyMutateRequest};

impl Method<Ethereum> for Mutate {
    type Returns = Option<ProposalWithApprovals>;

    const METHOD: &'static str = "mutate(((bytes32,bytes32,uint8,bytes),bytes32,bytes32,uint8))";

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

        let kind = SolRequestKind::from(&self.raw_request);

        let request_data = match self.raw_request {
            ProxyMutateRequest::Propose { proposal } => {
                SolProposal::try_from(proposal)?.abi_encode()
            }
            ProxyMutateRequest::Approve { approval } => {
                SolProposalApprovalWithSigner::from(approval).abi_encode()
            }
        };

        let sol_request = SolRequest {
            signerId: ecdsa_public_key,
            userId: user_id,
            kind,
            data: request_data.into(),
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

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded: super::super::ethereum::SolProposalWithApprovals =
            SolValue::abi_decode(&response, false)?;

        let proposal = ProposalWithApprovals {
            proposal_id: decoded.proposalId.rt().wrap_err("infallible conversion")?,
            num_approvals: decoded.numApprovals as usize,
        };

        Ok(Some(proposal))
    }
}
