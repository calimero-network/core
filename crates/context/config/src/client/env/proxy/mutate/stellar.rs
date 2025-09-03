#![cfg(feature = "stellar_client")]

//! Stellar specific implementations for context proxy mutations.
//!
//! This module provides Stellar blockchain-specific implementations of the
//! `Method<Stellar>` trait for context proxy mutation operations. It handles
//! Stellar's XDR (External Data Representation) serialization format using the `soroban_sdk` crate.
//!
//! ## Key Features
//!
//! - **XDR Serialization**: Uses Stellar's XDR format for parameter encoding and response decoding
//! - **ED25519 Signing**: Implements ED25519 signature generation and verification
//! - **Soroban Integration**: Leverages Soroban SDK for Stellar smart contract interactions
//! - **Error Handling**: Converts Stellar-specific errors to generic `eyre::Result`
//!
//! ## Implementation Details
//!
//! The mutation request is encoded using XDR serialization:
//! - Request data is converted to Stellar-specific types using `StellarProxyMutateRequest`
//! - Signatures are generated using ED25519 with XDR message encoding
//! - Responses are decoded using XDR's `FromXdr` trait with environment context
//! - Stellar-specific types are handled through dedicated wrapper types
//! - Environment context is managed through `Env::default()` for XDR operations
//!
//! ## Usage
//!
//! These implementations are used automatically by the `ContextProxyMutate` client
//! when the underlying transport is configured for Stellar. No direct usage is required.

use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::xdr::{FromXdr, ToXdr};
use soroban_sdk::{Bytes, Env};

use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::stellar::Stellar;
use crate::stellar::stellar_types::{
    FromWithEnv, StellarSignedRequest, StellarSignedRequestPayload,
};
use crate::stellar::{StellarProposalWithApprovals, StellarProxyMutateRequest};
use crate::ProposalWithApprovals;

impl Method<Stellar> for Mutate {
    type Returns = Option<ProposalWithApprovals>;

    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let payload: StellarProxyMutateRequest =
            StellarProxyMutateRequest::from_with_env(self.raw_request, &env);

        let signed_request_payload = StellarSignedRequestPayload::Proxy(payload);

        let signed_request =
            StellarSignedRequest::new(&env, signed_request_payload, |b| Ok(signer_sk.sign(b)))
                .map_err(|e| eyre::eyre!("Failed to sign request: {:?}", e))?;

        let bytes: Vec<u8> = signed_request.to_xdr(&env).into_iter().collect();

        Ok(bytes)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(None);
        }
        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let stellar_proposal = StellarProposalWithApprovals::from_xdr(&env, &env_bytes)
            .map_err(|_| eyre::eyre!("Failed to deserialize response"))?;

        let proposal: ProposalWithApprovals = stellar_proposal.into();

        Ok(Some(proposal))
    }
}
