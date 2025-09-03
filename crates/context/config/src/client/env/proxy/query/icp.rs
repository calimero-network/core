#![cfg(feature = "icp_client")]

//! Internet Computer (ICP) specific implementations for context proxy queries.
//!
//! This module provides Internet Computer blockchain-specific implementations of the
//! `Method<Icp>` trait for all context proxy query operations. It handles ICP's
//! Candid serialization format using the `candid` crate.
//!
//! ## Key Features
//!
//! - **Candid Serialization**: Uses Candid format for parameter encoding and response decoding
//! - **Type Safety**: Leverages Candid's type system for safe data serialization
//! - **Efficient Encoding**: Optimized for ICP's message passing and canister calls
//! - **Error Handling**: Converts ICP-specific errors to generic `eyre::Result`
//!
//! ## Implementation Details
//!
//! Each request type is encoded using Candid serialization:
//! - Simple types (u16, u32, Vec<u8>) are encoded directly with Candid
//! - Complex types use Candid's compound type encoding
//! - Responses are decoded using Candid's `Decode!` macro for type safety
//! - ICP-specific types are wrapped in `ICRepr` for proper serialization
//!
//! ## Usage
//!
//! These implementations are used automatically by the `ContextProxyQuery` client
//! when the underlying transport is configured for Internet Computer. No direct usage is required.

use candid::{Decode, Encode};

use super::super::requests::{
    ActiveProposalRequest, ContextStorageEntriesRequest, ContextVariableRequest,
    ProposalApprovalsRequest, ProposalApproversRequest, ProposalRequest, ProposalsRequest,
};
use crate::client::env::Method;
use crate::client::protocol::icp::Icp;
use crate::icp::repr::ICRepr;
use crate::icp::{ICProposal, ICProposalWithApprovals};
use crate::types::ContextIdentity;
use crate::{Proposal, ProposalWithApprovals};

impl Method<Icp> for ActiveProposalRequest {
    const METHOD: &'static str = "get_active_proposals_limit";
    type Returns = u16;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        Encode!(&self).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let value = Decode!(&response, u32)?;
        Ok(value as u16)
    }
}

impl Method<Icp> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";
    type Returns = Vec<crate::types::ContextStorageEntry>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        Encode!(&self.offset, &self.limit)
            .map_err(|e| eyre::eyre!("Failed to encode request: {}", e))
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let entries: Vec<(Vec<u8>, Vec<u8>)> = Decode!(&response, Vec<(Vec<u8>, Vec<u8>)>)
            .map_err(|e| eyre::eyre!("Failed to decode response: {}", e))?;
        Ok(entries
            .into_iter()
            .map(|(key, value)| crate::types::ContextStorageEntry { key, value })
            .collect())
    }
}

impl Method<Icp> for ContextVariableRequest {
    const METHOD: &'static str = "get_context_value";
    type Returns = Vec<u8>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let payload = ICRepr::new(self.key);
        Encode!(&payload).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, Vec<u8>)?;
        Ok(decoded)
    }
}

impl Method<Icp> for ProposalApprovalsRequest {
    const METHOD: &'static str = "get_confirmations_count";
    type Returns = ProposalWithApprovals;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let payload = ICRepr::new(*self.proposal_id);
        Encode!(&payload).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, ICProposalWithApprovals)?;
        Ok(decoded.into())
    }
}

impl Method<Icp> for ProposalApproversRequest {
    const METHOD: &'static str = "proposal_approvers";
    type Returns = Vec<ContextIdentity>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let payload = ICRepr::new(*self.proposal_id);
        Encode!(&payload).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let Some(identities) = Decode!(&response, Option<Vec<ICRepr<ContextIdentity>>>)? else {
            return Ok(Vec::new());
        };
        #[expect(
            clippy::transmute_undefined_repr,
            reason = "ICRepr<T> is a transparent wrapper around T"
        )]
        unsafe {
            Ok(std::mem::transmute::<
                Vec<ICRepr<ContextIdentity>>,
                Vec<ContextIdentity>,
            >(identities))
        }
    }
}

impl Method<Icp> for ProposalRequest {
    const METHOD: &'static str = "proposals";
    type Returns = Option<Proposal>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let payload = ICRepr::new(*self.proposal_id);
        Encode!(&payload).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, Option<ICProposal>)?;
        Ok(decoded.map(Into::into))
    }
}

impl Method<Icp> for ProposalsRequest {
    const METHOD: &'static str = "proposals";
    type Returns = Vec<Proposal>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        Encode!(&self.offset, &self.length).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let proposals = Decode!(&response, Vec<ICProposal>)?;
        Ok(proposals.into_iter().map(|id| id.into()).collect())
    }
}
