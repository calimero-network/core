#![cfg(feature = "icp_client")]

//! Internet Computer (ICP) specific implementations for context proxy mutations.
//!
//! This module provides Internet Computer blockchain-specific implementations of the
//! `Method<Icp>` trait for context proxy mutation operations. It handles ICP's
//! Candid serialization format using the `candid` crate.
//!
//! ## Key Features
//!
//! - **Candid Serialization**: Uses Candid format for parameter encoding and response decoding
//! - **ED25519 Signing**: Implements ED25519 signature generation and verification
//! - **Type Safety**: Leverages Candid's type system for safe data serialization
//! - **Error Handling**: Converts ICP-specific errors to generic `eyre::Result`
//!
//! ## Implementation Details
//!
//! The mutation request is encoded using Candid serialization:
//! - Request data is converted to ICP-specific types using `ICProxyMutateRequest`
//! - Signatures are generated using ED25519 with Candid message encoding
//! - Responses are decoded using Candid's `Decode!` macro for type safety
//! - ICP-specific types are wrapped in `ICSigned` for proper serialization
//!
//! ## Usage
//!
//! These implementations are used automatically by the `ContextProxyMutate` client
//! when the underlying transport is configured for Internet Computer. No direct usage is required.

use candid::Decode;
use ed25519_dalek::{Signer, SigningKey};

use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::icp::Icp;
use crate::icp::types::ICSigned;
use crate::icp::{ICProposalWithApprovals, ICProxyMutateRequest};
use crate::ProposalWithApprovals;

impl Method<Icp> for Mutate {
    type Returns = Option<ProposalWithApprovals>;

    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let payload: ICProxyMutateRequest =
            self.raw_request.try_into().map_err(eyre::Report::msg)?;

        let signed = ICSigned::new(payload, |b| signer_sk.sign(b))?;

        let encoded = candid::encode_one(&signed)?;

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, Option<ICProposalWithApprovals>)?;
        Ok(decoded.map(Into::into))
    }
}
