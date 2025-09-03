#![cfg(feature = "starknet_client")]

//! Starknet specific implementations for context proxy queries.
//!
//! This module provides Starknet blockchain-specific implementations of the
//! `Method<Starknet>` trait for all context proxy query operations. It handles
//! Starknet's Cairo serialization format using the `starknet_core` and `starknet_crypto` crates.
//!
//! ## Key Features
//!
//! - **Cairo Serialization**: Uses Cairo's native serialization for parameter encoding and response decoding
//! - **Felt Encoding**: Leverages Starknet's `Felt` type for efficient field element handling
//! - **Call Data**: Uses `CallData` for structured parameter encoding in smart contract calls
//! - **Error Handling**: Converts Starknet-specific errors to generic `eyre::Result`
//!
//! ## Implementation Details
//!
//! Each request type is encoded using Cairo serialization:
//! - Simple types (u16, u32) are converted to `Felt` and encoded
//! - Complex types use `CallData` for structured encoding
//! - Responses are decoded using Cairo's `Decode` trait implementations
//! - Starknet-specific types are handled through dedicated wrapper types
//! - 32-byte chunks are processed for proper field element alignment
//!
//! ## Usage
//!
//! These implementations are used automatically by the `ContextProxyQuery` client
//! when the underlying transport is configured for Starknet. No direct usage is required.

use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode};
use starknet::core::types::Felt;
use starknet_crypto::Felt as CryptoFelt;

use super::{
    ActiveProposalRequest, ContextStorageEntriesRequest, ContextVariableRequest,
    ProposalApprovalsRequest, ProposalApproversRequest, ProposalRequest, ProposalsRequest,
};
use crate::client::env::proxy::starknet::{
    CallData, ContextStorageEntriesResponse, StarknetContextStorageEntriesRequest,
};
use crate::client::env::proxy::types::starknet::{
    StarknetApprovers, StarknetProposal, StarknetProposalId, StarknetProposalWithApprovals,
};
use crate::client::env::Method;
use crate::client::protocol::starknet::Starknet;
use crate::types::{ContextIdentity, ContextStorageEntry};
use crate::{Proposal, ProposalWithApprovals};

impl Method<Starknet> for ActiveProposalRequest {
    const METHOD: &'static str = "get_active_proposals_limit";
    type Returns = u16;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        Ok(Vec::new())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 32 bytes, got {}",
                response.len()
            ));
        }
        if !response[..30].iter().all(|&b| b == 0) {
            return Err(eyre::eyre!(
                "Invalid response format: non-zero bytes in prefix"
            ));
        }
        Ok(u16::from_be_bytes([response[30], response[31]]))
    }
}

impl Method<Starknet> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";
    type Returns = Vec<ContextStorageEntry>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let req = StarknetContextStorageEntriesRequest {
            offset: CryptoFelt::from(self.offset as u64),
            length: CryptoFelt::from(self.limit as u64),
        };
        let mut call_data = CallData::default();
        req.encode(&mut call_data)?;
        Ok(call_data.0)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(vec![]);
        }
        let chunks = response.chunks_exact(32);
        let felts: Vec<CryptoFelt> = chunks
            .map(|chunk| {
                let chunk_array: [u8; 32] = chunk
                    .try_into()
                    .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
                Ok(CryptoFelt::from_bytes_be(&chunk_array))
            })
            .collect::<eyre::Result<Vec<CryptoFelt>>>()?;
        let response = ContextStorageEntriesResponse::decode_iter(&mut felts.iter())?;
        Ok(response.entries.into_iter().map(Into::into).collect())
    }
}

impl Method<Starknet> for ContextVariableRequest {
    const METHOD: &'static str = "get_context_value";
    type Returns = Vec<u8>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();
        let key: crate::client::env::proxy::starknet::ContextVariableKey = self.key.into();
        key.encode(&mut call_data)?;
        Ok(call_data.0)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(vec![]);
        }
        let chunks = response.chunks_exact(32);
        let felts: Vec<CryptoFelt> = chunks
            .map(|chunk| {
                let chunk_array: [u8; 32] = chunk
                    .try_into()
                    .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
                Ok(CryptoFelt::from_bytes_be(&chunk_array))
            })
            .collect::<eyre::Result<Vec<CryptoFelt>>>()?;
        if felts.is_empty() {
            return Ok(vec![]);
        }
        match felts[0] {
            f if f == CryptoFelt::ZERO => Ok(response[64..]
                .iter()
                .filter(|&&b| b != 0)
                .copied()
                .collect()),
            v => Err(eyre::eyre!("Invalid option discriminant: {}", v)),
        }
    }
}

impl Method<Starknet> for ProposalApprovalsRequest {
    const METHOD: &'static str = "get_confirmations_count";
    type Returns = ProposalWithApprovals;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let starknet_id: StarknetProposalId = self.proposal_id.into();
        let mut call_data = CallData::default();
        starknet_id.encode(&mut call_data)?;
        Ok(call_data.0)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("Empty response"));
        }
        if response.len() % 32 != 0 {
            return Err(eyre::eyre!(
                "Invalid response length: {} bytes is not a multiple of 32",
                response.len()
            ));
        }
        let mut felts = Vec::new();
        let chunks = response.chunks_exact(32);
        if !chunks.remainder().is_empty() {
            return Err(eyre::eyre!("Response length is not a multiple of 32 bytes"));
        }
        for chunk in chunks {
            let chunk_array: [u8; 32] = chunk
                .try_into()
                .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
            felts.push(Felt::from_bytes_be(&chunk_array));
        }
        let approvals = StarknetProposalWithApprovals::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode approvals: {:?}", e))?;
        Ok(approvals.into())
    }
}

impl Method<Starknet> for ProposalApproversRequest {
    const METHOD: &'static str = "proposal_approvers";
    type Returns = Vec<ContextIdentity>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let starknet_id: StarknetProposalId = self.proposal_id.into();
        let mut call_data = CallData::default();
        starknet_id.encode(&mut call_data)?;
        Ok(call_data.0)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(Vec::new());
        }
        if response.len() % 32 != 0 {
            return Err(eyre::eyre!(
                "Invalid response length: {} bytes is not a multiple of 32",
                response.len()
            ));
        }
        let mut felts = Vec::new();
        let chunks = response.chunks_exact(32);
        if !chunks.remainder().is_empty() {
            return Err(eyre::eyre!("Response length is not a multiple of 32 bytes"));
        }
        for chunk in chunks {
            let chunk_array: [u8; 32] = chunk
                .try_into()
                .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
            felts.push(Felt::from_bytes_be(&chunk_array));
        }
        let approvers = StarknetApprovers::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode approvers: {:?}", e))?;
        Ok(approvers.into())
    }
}

impl Method<Starknet> for ProposalRequest {
    const METHOD: &'static str = "proposal";
    type Returns = Option<Proposal>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let starknet_id: StarknetProposalId = self.proposal_id.into();
        let mut call_data = CallData::default();
        starknet_id.encode(&mut call_data)?;
        Ok(call_data.0)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(None);
        }
        if response.len() % 32 != 0 {
            return Err(eyre::eyre!(
                "Invalid response length: {} bytes is not a multiple of 32",
                response.len()
            ));
        }
        let mut felts = Vec::new();
        let chunks = response.chunks_exact(32);
        if !chunks.remainder().is_empty() {
            return Err(eyre::eyre!("Response length is not a multiple of 32 bytes"));
        }
        for chunk in chunks {
            let chunk_array: [u8; 32] = chunk
                .try_into()
                .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
            felts.push(Felt::from_bytes_be(&chunk_array));
        }
        if felts.is_empty() {
            return Ok(None);
        }
        match felts[0].to_bytes_be()[31] {
            0 => Ok(None),
            1 => {
                let proposal = StarknetProposal::decode(&felts[1..])
                    .map_err(|e| eyre::eyre!("Failed to decode proposal: {:?}", e))?;
                Ok(Some(proposal.into()))
            }
            v => Err(eyre::eyre!("Invalid option discriminant: {}", v)),
        }
    }
}

impl Method<Starknet> for ProposalsRequest {
    const METHOD: &'static str = "proposals";
    type Returns = Vec<Proposal>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let req = crate::client::env::proxy::starknet::StarknetProposalsRequest {
            offset: Felt::from(self.offset as u64),
            length: Felt::from(self.length as u64),
        };
        let mut call_data = CallData::default();
        req.encode(&mut call_data)?;
        Ok(call_data.0)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(Vec::new());
        }
        if response.len() % 32 != 0 {
            return Err(eyre::eyre!(
                "Invalid response length: {} bytes is not a multiple of 32",
                response.len()
            ));
        }
        let mut felts = Vec::new();
        let chunks = response.chunks_exact(32);
        if !chunks.remainder().is_empty() {
            return Err(eyre::eyre!("Response length is not a multiple of 32 bytes"));
        }
        for chunk in chunks {
            let chunk_array: [u8; 32] = chunk
                .try_into()
                .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
            felts.push(Felt::from_bytes_be(&chunk_array));
        }
        if felts.is_empty() {
            return Ok(Vec::new());
        }
        let proposals = crate::client::env::proxy::starknet::StarknetProposals::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode proposals: {:?}", e))?;
        Ok(proposals.into())
    }
}
