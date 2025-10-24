#![cfg(feature = "ethereum_client")]

//! Ethereum-specific implementations for context proxy queries.

use alloy::dyn_abi::{DynSolType, DynSolValue};
use alloy::primitives::B256;
use alloy_sol_types::SolValue;

use super::super::requests::{
    ActiveProposalRequest, ContextStorageEntriesRequest, ContextVariableRequest,
    ProposalApprovalsRequest, ProposalApproversRequest, ProposalRequest, ProposalsRequest,
};
use crate::client::env::proxy::ethereum::SolProposal;
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::repr::ReprTransmute;
use crate::types::{ContextIdentity, ContextStorageEntry};
use crate::{Proposal, ProposalWithApprovals};

impl Method<Ethereum> for ActiveProposalRequest {
    type Returns = u16;
    const METHOD: &'static str = "getActiveProposalsLimit()";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        Ok(().abi_encode())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let active_proposals_limit: Self::Returns = SolValue::abi_decode(&response)?;
        Ok(active_proposals_limit)
    }
}

impl Method<Ethereum> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "contextStorageEntries(uint32,uint32)";
    type Returns = Vec<ContextStorageEntry>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let offset = u32::try_from(self.offset)
            .map_err(|e| eyre::eyre!("Offset too large for u32: {}", e))?;
        let limit =
            u32::try_from(self.limit).map_err(|e| eyre::eyre!("Limit too large for u32: {}", e))?;
        Ok((offset, limit).abi_encode())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let struct_type = "tuple(bytes,bytes)[]".parse::<DynSolType>()?;
        let decoded = struct_type.abi_decode(&response)?;
        let DynSolValue::Array(entries) = decoded else {
            return Err(eyre::eyre!("Expected array"));
        };
        Ok(entries
            .into_iter()
            .map(|entry| {
                let DynSolValue::Tuple(fields) = entry else {
                    return Err(eyre::eyre!("Expected tuple"));
                };
                let all_bytes = fields[1]
                    .as_bytes()
                    .ok_or_else(|| eyre::eyre!("Failed to get bytes from field"))?;

                let key_len = all_bytes[31] as usize;
                let key = all_bytes[32..32 + key_len].to_vec();

                #[allow(clippy::integer_division, reason = "Need this for 32-byte alignment")]
                let value_offset = 32 + ((key_len + 31) / 32) * 32;
                let value_len = all_bytes[value_offset + 31] as usize;
                let value = all_bytes[value_offset + 32..value_offset + 32 + value_len].to_vec();

                Ok(ContextStorageEntry { key, value })
            })
            .collect::<Result<Vec<_>, _>>()?)
    }
}

impl Method<Ethereum> for ContextVariableRequest {
    type Returns = Vec<u8>;
    const METHOD: &'static str = "getContextValue(bytes)";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        Ok(self.key.abi_encode())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let context_value: Self::Returns = SolValue::abi_decode(&response)?;
        Ok(context_value)
    }
}

impl Method<Ethereum> for ProposalApprovalsRequest {
    type Returns = ProposalWithApprovals;
    const METHOD: &'static str = "getConfirmationsCount(bytes32)";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let proposal_id: [u8; 32] = self
            .proposal_id
            .rt()
            .map_err(|e| eyre::eyre!("Failed to convert proposal_id: {}", e))?;

        Ok(proposal_id.abi_encode())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let (proposal_id, num_approvals): (B256, u32) = SolValue::abi_decode(&response)?;

        Ok(ProposalWithApprovals {
            proposal_id: proposal_id
                .rt()
                .map_err(|e| eyre::eyre!("Failed to convert proposal_id: {}", e))?,
            num_approvals: num_approvals as usize,
        })
    }
}

impl Method<Ethereum> for ProposalApproversRequest {
    type Returns = Vec<ContextIdentity>;
    const METHOD: &'static str = "proposalApprovers(bytes32)";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let proposal_id: [u8; 32] = self
            .proposal_id
            .rt()
            .map_err(|e| eyre::eyre!("Failed to convert proposal_id: {}", e))?;
        Ok(proposal_id.abi_encode())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded: Vec<B256> = SolValue::abi_decode(&response)?;
        let context_identities: Result<Vec<ContextIdentity>, _> = decoded
            .into_iter()
            .map(|bytes| {
                bytes
                    .rt()
                    .map_err(|e| eyre::eyre!("Failed to convert bytes: {}", e))
            })
            .collect();
        Ok(context_identities?)
    }
}

impl Method<Ethereum> for ProposalRequest {
    type Returns = Option<Proposal>;
    const METHOD: &'static str = "getProposal(bytes32)";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let proposal_id: [u8; 32] = self
            .proposal_id
            .rt()
            .map_err(|e| eyre::eyre!("Failed to convert proposal_id: {}", e))?;
        Ok(proposal_id.abi_encode())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() || response.iter().all(|&b| b == 0) {
            return Ok(None);
        }
        let sol_proposal: SolProposal = SolValue::abi_decode(&response)?;
        sol_proposal.try_into().map(Some)
    }
}

impl Method<Ethereum> for ProposalsRequest {
    type Returns = Vec<Proposal>;
    const METHOD: &'static str = "getProposals(uint32,uint32)";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let offset = u32::try_from(self.offset)
            .map_err(|e| eyre::eyre!("Offset too large for u32: {}", e))?;
        let length = u32::try_from(self.length)
            .map_err(|e| eyre::eyre!("Limit too large for u32: {}", e))?;
        Ok((offset, length).abi_encode())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let proposals: Vec<SolProposal> = SolValue::abi_decode(&response)?;
        proposals.into_iter().map(TryInto::try_into).collect()
    }
}
