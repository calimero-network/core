use crate::client::env::utils;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::repr::Repr;
use crate::types::{ContextIdentity, ContextStorageEntry};
use crate::{Proposal, ProposalId, ProposalWithApprovals};
use std::io::Cursor;
use alloy::dyn_abi::{DynSolType, DynSolValue};
use alloy::primitives::B256;
use alloy_sol_types::SolValue;
use candid::{CandidType, Decode, Encode};
use eyre::{eyre, WrapErr};
use serde::{Deserialize, Serialize};
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{Bytes, BytesN, Env, IntoVal, TryFromVal, TryIntoVal, Val};
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode};
use starknet::core::types::Felt;
use starknet_crypto::Felt as CryptoFelt;

use crate::client::env::proxy::ethereum::SolProposal;
use crate::client::env::proxy::starknet::{
    CallData,
    ContextStorageEntriesResponse,
    StarknetContextStorageEntriesRequest,
    StarknetProposals,
    StarknetProposalsRequest,
};
use crate::client::env::proxy::types::starknet::{
    StarknetApprovers,
    StarknetProposal,
    StarknetProposalId,
    StarknetProposalWithApprovals,
};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::repr::ICRepr;
use crate::icp::{ICProposal, ICProposalWithApprovals};
use crate::repr::{ReprTransmute, Repr as ReprType};
use crate::stellar::{StellarProposal, StellarProposalWithApprovals};

#[derive(Debug)]
pub struct ContextProxyQuery<'a, T> {
    pub client: CallClient<'a, T>,
}

impl<'a, T: Transport> ContextProxyQuery<'a, T> {
    pub async fn proposals(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Proposal>, ClientError<T>> {
        let params = ProposalsRequest {
            offset,
            length: limit,
        };
        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn proposal(
        &self,
        proposal_id: ProposalId,
    ) -> Result<Option<Proposal>, ClientError<T>> {
        let params = ProposalRequest {
            proposal_id: Repr::new(proposal_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_number_of_active_proposals(&self) -> Result<u16, ClientError<T>> {
        let params = ActiveProposalRequest;

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_number_of_proposal_approvals(
        &self,
        proposal_id: ProposalId,
    ) -> Result<ProposalWithApprovals, ClientError<T>> {
        let params = ProposalApprovalsRequest {
            proposal_id: Repr::new(proposal_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_proposal_approvers(
        &self,
        proposal_id: ProposalId,
    ) -> Result<Vec<ContextIdentity>, ClientError<T>> {
        let params = ProposalApproversRequest {
            proposal_id: Repr::new(proposal_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_context_value(&self, key: Vec<u8>) -> Result<Vec<u8>, ClientError<T>> {
        let params = ContextVariableRequest { key };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_context_storage_entries(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ContextStorageEntry>, ClientError<T>> {
        let params = ContextStorageEntriesRequest { offset, limit };

        utils::send(&self.client, Operation::Read(params)).await
    }
}

// Inlined from active_proposals.rs
#[derive(Copy, Clone, Debug, Serialize, CandidType)]
pub(super) struct ActiveProposalRequest;

impl Method<Near> for ActiveProposalRequest {
    const METHOD: &'static str = "get_active_proposals_limit";
    type Returns = u16;
    fn encode(self) -> eyre::Result<Vec<u8>> { serde_json::to_vec(&self).map_err(Into::into) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { serde_json::from_slice(&response).map_err(Into::into) }
}

impl Method<Starknet> for ActiveProposalRequest {
    const METHOD: &'static str = "get_active_proposals_limit";
    type Returns = u16;
    fn encode(self) -> eyre::Result<Vec<u8>> { Ok(Vec::new()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 32 { return Err(eyre!("Invalid response length: expected 32 bytes, got {}", response.len())); }
        if !response[..30].iter().all(|&b| b == 0) { return Err(eyre!("Invalid response format: non-zero bytes in prefix")); }
        Ok(u16::from_be_bytes([response[30], response[31]]))
    }
}

impl Method<Icp> for ActiveProposalRequest {
    const METHOD: &'static str = "get_active_proposals_limit";
    type Returns = u16;
    fn encode(self) -> eyre::Result<Vec<u8>> { Encode!(&self).map_err(Into::into) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let value = Decode!(&response, u32)?; Ok(value as u16) }
}

impl Method<Stellar> for ActiveProposalRequest {
    type Returns = u16;
    const METHOD: &'static str = "get_active_proposals_limit";
    fn encode(self) -> eyre::Result<Vec<u8>> { Ok(Vec::new()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());
        let sc_val = ScVal::read_xdr(&mut limited).map_err(|e| eyre!("Failed to read XDR: {}", e))?;
        let active_proposals_limit: u32 = sc_val.try_into().map_err(|e| eyre!("Failed to convert to u64: {:?}", e))?;
        Ok(active_proposals_limit as u16)
    }
}

impl Method<Ethereum> for ActiveProposalRequest {
    type Returns = u16;
    const METHOD: &'static str = "getActiveProposalsLimit()";
    fn encode(self) -> eyre::Result<Vec<u8>> { Ok(().abi_encode()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { Ok(SolValue::abi_decode(&response, false)?) }
}

// Inlined from context_storage_entries.rs
#[derive(Clone, Debug, Serialize)]
pub(super) struct ContextStorageEntriesRequest { pub(super) offset: usize, pub(super) limit: usize }

impl Method<Near> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";
    type Returns = Vec<ContextStorageEntry>;
    fn encode(self) -> eyre::Result<Vec<u8>> { serde_json::to_vec(&self).map_err(Into::into) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let entries: Vec<(Box<[u8]>, Box<[u8]>)> = serde_json::from_slice(&response).map_err(|e| eyre!("Failed to decode response: {}", e))?;
        Ok(entries.into_iter().map(|(key, value)| ContextStorageEntry { key: key.into(), value: value.into() }).collect())
    }
}

impl Method<Starknet> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";
    type Returns = Vec<ContextStorageEntry>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let req = StarknetContextStorageEntriesRequest { offset: CryptoFelt::from(self.offset as u64), length: CryptoFelt::from(self.limit as u64) };
        let mut call_data = CallData::default();
        req.encode(&mut call_data)?; Ok(call_data.0)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() { return Ok(vec![]); }
        let chunks = response.chunks_exact(32);
        let felts: Vec<CryptoFelt> = chunks.map(|chunk| { let chunk_array: [u8; 32] = chunk.try_into().map_err(|e| eyre!("Failed to convert chunk to array: {}", e))?; Ok(CryptoFelt::from_bytes_be(&chunk_array)) }).collect::<eyre::Result<Vec<CryptoFelt>>>()?;
        let response = ContextStorageEntriesResponse::decode_iter(&mut felts.iter())?;
        Ok(response.entries.into_iter().map(Into::into).collect())
    }
}

impl Method<Icp> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";
    type Returns = Vec<ContextStorageEntry>;
    fn encode(self) -> eyre::Result<Vec<u8>> { Encode!(&self.offset, &self.limit).map_err(|e| eyre!("Failed to encode request: {}", e)) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let entries: Vec<(Vec<u8>, Vec<u8>)> = Decode!(&response, Vec<(Vec<u8>, Vec<u8>)>).map_err(|e| eyre!("Failed to decode response: {}", e))?;
        Ok(entries.into_iter().map(|(key, value)| ContextStorageEntry { key, value }).collect())
    }
}

impl Method<Stellar> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";
    type Returns = Vec<ContextStorageEntry>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default(); let offset_val: u32 = self.offset as u32; let limit_val: u32 = self.limit as u32; let args = (offset_val, limit_val); let xdr = args.to_xdr(&env); Ok(xdr.to_alloc_vec())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response); let mut limited = Limited::new(cursor, Limits::none()); let sc_val = ScVal::read_xdr(&mut limited).map_err(|e| eyre!("Failed to read XDR: {}", e))?; let env = Env::default(); let entries: soroban_sdk::Vec<(Bytes, Bytes)> = sc_val.try_into_val(&env).map_err(|e| eyre!("Failed to convert to entries: {:?}", e))?; Ok(entries.iter().map(|(key, value)| ContextStorageEntry { key: key.to_alloc_vec(), value: value.to_alloc_vec() }).collect())
    }
}

impl Method<Ethereum> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "contextStorageEntries(uint32,uint32)";
    type Returns = Vec<ContextStorageEntry>;
    fn encode(self) -> eyre::Result<Vec<u8>> { let offset = u32::try_from(self.offset).map_err(|e| eyre!("Offset too large for u32: {}", e))?; let limit = u32::try_from(self.limit).map_err(|e| eyre!("Limit too large for u32: {}", e))?; Ok((offset, limit).abi_encode()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let struct_type = "tuple(bytes,bytes)[]".parse::<DynSolType>()?; let decoded = struct_type.abi_decode(&response)?; let DynSolValue::Array(entries) = decoded else { return Err(eyre!("Expected array")); };
        Ok(entries.into_iter().map(|entry| { let DynSolValue::Tuple(fields) = entry else { return Err(eyre!("Expected tuple")); }; let all_bytes = fields[1].as_bytes().ok_or_else(|| eyre!("Failed to get bytes from field"))?; let key_len = all_bytes[31] as usize; let key = all_bytes[32..32 + key_len].to_vec(); #[allow(clippy::integer_division)] let value_offset = 32 + ((key_len + 31) / 32) * 32; let value_len = all_bytes[value_offset + 31] as usize; let value = all_bytes[value_offset + 32..value_offset + 32 + value_len].to_vec(); Ok(ContextStorageEntry { key, value }) }).collect::<Result<Vec<_>, _>>()?)
    }
}

// Inlined from context_variable.rs
#[derive(Clone, Debug, Serialize)]
pub(super) struct ContextVariableRequest { pub(super) key: Vec<u8> }

impl Method<Near> for ContextVariableRequest {
    const METHOD: &'static str = "get_context_value"; type Returns = Vec<u8>;
    fn encode(self) -> eyre::Result<Vec<u8>> { serde_json::to_vec(&self).map_err(Into::into) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { serde_json::from_slice(&response).map_err(Into::into) }
}

impl Method<Starknet> for ContextVariableRequest {
    const METHOD: &'static str = "get_context_value"; type Returns = Vec<u8>;
    fn encode(self) -> eyre::Result<Vec<u8>> { let mut call_data = CallData::default(); let key: crate::client::env::proxy::starknet::ContextVariableKey = self.key.into(); key.encode(&mut call_data)?; Ok(call_data.0) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() { return Ok(vec![]); }
        let chunks = response.chunks_exact(32); let felts: Vec<CryptoFelt> = chunks.map(|chunk| { let chunk_array: [u8; 32] = chunk.try_into().map_err(|e| eyre!("Failed to convert chunk to array: {}", e))?; Ok(CryptoFelt::from_bytes_be(&chunk_array)) }).collect::<eyre::Result<Vec<CryptoFelt>>>()?; if felts.is_empty() { return Ok(vec![]); }
        match felts[0] { f if f == CryptoFelt::ZERO => { Ok(response[64..].iter().filter(|&&b| b != 0).copied().collect()) } v => Err(eyre!("Invalid option discriminant: {}", v)), }
    }
}

impl Method<Icp> for ContextVariableRequest {
    const METHOD: &'static str = "get_context_value"; type Returns = Vec<u8>;
    fn encode(self) -> eyre::Result<Vec<u8>> { let payload = ICRepr::new(self.key); Encode!(&payload).map_err(Into::into) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let decoded = Decode!(&response, Vec<u8>)?; Ok(decoded) }
}

impl Method<Stellar> for ContextVariableRequest {
    type Returns = Vec<u8>; const METHOD: &'static str = "get_context_value";
    fn encode(self) -> eyre::Result<Vec<u8>> { let env = Env::default(); let key_val: Bytes = Bytes::from_slice(&env, &self.key); let args = (key_val,); let xdr = args.to_xdr(&env); Ok(xdr.to_alloc_vec()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let cursor = Cursor::new(response); let mut limited = Limited::new(cursor, Limits::none()); let sc_val = ScVal::read_xdr(&mut limited).map_err(|e| eyre!("Failed to read XDR: {}", e))?; if sc_val == ScVal::Void { return Ok(Vec::new()); } let env = Env::default(); let value: Bytes = sc_val.try_into_val(&env).map_err(|e| eyre!("Failed to convert to Bytes: {:?}", e))?; Ok(value.to_alloc_vec()) }
}

impl Method<Ethereum> for ContextVariableRequest {
    type Returns = Vec<u8>; const METHOD: &'static str = "getContextValue(bytes)";
    fn encode(self) -> eyre::Result<Vec<u8>> { Ok(self.key.abi_encode()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { Ok(SolValue::abi_decode(&response, false)?) }
}

// Inlined from proposal_approvals.rs
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ProposalApprovalsRequest { pub(super) proposal_id: Repr<ProposalId> }

impl Method<Near> for ProposalApprovalsRequest {
    const METHOD: &'static str = "get_confirmations_count"; type Returns = ProposalWithApprovals;
    fn encode(self) -> eyre::Result<Vec<u8>> { serde_json::to_vec(&self).map_err(Into::into) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { serde_json::from_slice(&response).map_err(Into::into) }
}

impl Method<Starknet> for ProposalApprovalsRequest {
    const METHOD: &'static str = "get_confirmations_count"; type Returns = ProposalWithApprovals;
    fn encode(self) -> eyre::Result<Vec<u8>> { let starknet_id: StarknetProposalId = self.proposal_id.into(); let mut call_data = CallData::default(); starknet_id.encode(&mut call_data)?; Ok(call_data.0) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() { return Err(eyre!("Empty response")); }
        if response.len() % 32 != 0 { return Err(eyre!("Invalid response length: {} bytes is not a multiple of 32", response.len())); }
        let mut felts = Vec::new(); let chunks = response.chunks_exact(32); if !chunks.remainder().is_empty() { return Err(eyre!("Response length is not a multiple of 32 bytes")); }
        for chunk in chunks { let chunk_array: [u8; 32] = chunk.try_into().map_err(|e| eyre!("Failed to convert chunk to array: {}", e))?; felts.push(Felt::from_bytes_be(&chunk_array)); }
        let approvals = StarknetProposalWithApprovals::decode(&felts).map_err(|e| eyre!("Failed to decode approvals: {:?}", e))?; Ok(approvals.into())
    }
}

impl Method<Icp> for ProposalApprovalsRequest {
    const METHOD: &'static str = "get_confirmations_count"; type Returns = ProposalWithApprovals;
    fn encode(self) -> eyre::Result<Vec<u8>> { let payload = ICRepr::new(*self.proposal_id); Encode!(&payload).map_err(Into::into) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let decoded = Decode!(&response, ICProposalWithApprovals)?; Ok(decoded.into()) }
}

impl Method<Stellar> for ProposalApprovalsRequest {
    type Returns = ProposalWithApprovals; const METHOD: &'static str = "get_confirmations_count";
    fn encode(self) -> eyre::Result<Vec<u8>> { let env = Env::default(); let proposal_id_raw: [u8; 32] = self.proposal_id.rt().map_err(|e| eyre!("cannot convert proposal id to raw bytes: {}", e))?; let proposal_id_val: BytesN<32> = proposal_id_raw.into_val(&env); let args = (proposal_id_val,); let xdr = args.to_xdr(&env); Ok(xdr.to_alloc_vec()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let cursor = Cursor::new(response); let mut limited = Limited::new(cursor, Limits::none()); let sc_val = ScVal::read_xdr(&mut limited).map_err(|e| eyre!("Failed to read XDR: {}", e))?; if sc_val == ScVal::Void { return Err(eyre!("Proposal not found")); } let env = Env::default(); let val: Val = sc_val.try_into_val(&env).map_err(|e| eyre!("Failed to convert ScVal to Val: {:?}", e))?; let stellar_proposal: StellarProposalWithApprovals = val.try_into_val(&env).map_err(|e| eyre!("Failed to convert to StellarProposalWithApprovals: {:?}", e))?; Ok(ProposalWithApprovals::from(stellar_proposal)) }
}

impl Method<Ethereum> for ProposalApprovalsRequest {
    type Returns = ProposalWithApprovals; const METHOD: &'static str = "getConfirmationsCount(bytes32)";
    fn encode(self) -> eyre::Result<Vec<u8>> { let proposal_id: [u8; 32] = self.proposal_id.rt().map_err(|e| eyre!("Failed to convert proposal_id: {}", e))?; Ok(proposal_id.abi_encode()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let (proposal_id, num_approvals): (B256, u32) = SolValue::abi_decode(&response, false)?; Ok(ProposalWithApprovals { proposal_id: proposal_id.rt().wrap_err("infallible conversion")?, num_approvals: num_approvals as usize }) }
}

// Inlined from proposal_approvers.rs
#[derive(Clone, Debug, Serialize)]
pub(super) struct ProposalApproversRequest { pub(super) proposal_id: Repr<ProposalId> }

impl Method<Near> for ProposalApproversRequest {
    const METHOD: &'static str = "get_proposal_approvers"; type Returns = Vec<ContextIdentity>;
    fn encode(self) -> eyre::Result<Vec<u8>> { serde_json::to_vec(&self).map_err(Into::into) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let members: Vec<Repr<ContextIdentity>> = serde_json::from_slice(&response)?;
        #[expect(clippy::transmute_undefined_repr, reason = "Repr<T> is a transparent wrapper around T")]
        let members = unsafe { std::mem::transmute::<Vec<Repr<ContextIdentity>>, Vec<ContextIdentity>>(members) };
        Ok(members)
    }
}

impl Method<Starknet> for ProposalApproversRequest {
    const METHOD: &'static str = "proposal_approvers"; type Returns = Vec<ContextIdentity>;
    fn encode(self) -> eyre::Result<Vec<u8>> { let starknet_id: StarknetProposalId = self.proposal_id.into(); let mut call_data = CallData::default(); starknet_id.encode(&mut call_data)?; Ok(call_data.0) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() { return Ok(Vec::new()); }
        if response.len() % 32 != 0 { return Err(eyre!("Invalid response length: {} bytes is not a multiple of 32", response.len())); }
        let mut felts = Vec::new(); let chunks = response.chunks_exact(32); if !chunks.remainder().is_empty() { return Err(eyre!("Response length is not a multiple of 32 bytes")); }
        for chunk in chunks { let chunk_array: [u8; 32] = chunk.try_into().map_err(|e| eyre!("Failed to convert chunk to array: {}", e))?; felts.push(Felt::from_bytes_be(&chunk_array)); }
        let approvers = StarknetApprovers::decode(&felts).map_err(|e| eyre!("Failed to decode approvers: {:?}", e))?; Ok(approvers.into())
    }
}

impl Method<Icp> for ProposalApproversRequest {
    const METHOD: &'static str = "proposal_approvers"; type Returns = Vec<ContextIdentity>;
    fn encode(self) -> eyre::Result<Vec<u8>> { let payload = ICRepr::new(*self.proposal_id); Encode!(&payload).map_err(Into::into) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let Some(identities) = Decode!(&response, Option<Vec<ICRepr<ContextIdentity>>>)? else { return Ok(Vec::new()); }; #[expect(clippy::transmute_undefined_repr, reason = "ICRepr<T> is a transparent wrapper around T")] unsafe { Ok(std::mem::transmute::<Vec<ICRepr<ContextIdentity>>, Vec<ContextIdentity>>(identities)) }
}

impl Method<Stellar> for ProposalApproversRequest {
    type Returns = Vec<ContextIdentity>; const METHOD: &'static str = "proposal_approvers";
    fn encode(self) -> eyre::Result<Vec<u8>> { let env = Env::default(); let proposal_id_raw: [u8; 32] = self.proposal_id.rt().expect("infallible conversion"); let proposal_id_val: BytesN<32> = proposal_id_raw.into_val(&env); let args = (proposal_id_val,); let xdr = args.to_xdr(&env); Ok(xdr.to_alloc_vec()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let cursor = Cursor::new(response); let mut limited = Limited::new(cursor, Limits::none()); let sc_val = ScVal::read_xdr(&mut limited).map_err(|e| eyre!("Failed to read XDR: {}", e))?; if sc_val == ScVal::Void { return Ok(Vec::new()); } let env = Env::default(); let approvers: soroban_sdk::Vec<BytesN<32>> = sc_val.try_into_val(&env).map_err(|e| eyre!("Failed to convert to approvers: {:?}", e))?; approvers.iter().map(|bytes| bytes.to_array().rt().map_err(|e| eyre!("Failed to convert bytes to identity: {}", e))).collect() }
}

impl Method<Ethereum> for ProposalApproversRequest {
    type Returns = Vec<ContextIdentity>; const METHOD: &'static str = "proposalApprovers(bytes32)";
    fn encode(self) -> eyre::Result<Vec<u8>> { let proposal_id: [u8; 32] = self.proposal_id.rt().expect("infallible conversion"); Ok(proposal_id.abi_encode()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let decoded: Vec<B256> = SolValue::abi_decode(&response, false)?; Ok(decoded.into_iter().map(|bytes| bytes.rt().expect("infallible conversion")).collect()) }
}

// Inlined from proposal.rs
#[derive(Clone, Debug, Serialize)]
pub(super) struct ProposalRequest { pub(super) proposal_id: Repr<ProposalId> }

impl Method<Near> for ProposalRequest { const METHOD: &'static str = "proposal"; type Returns = Option<Proposal>; fn encode(self) -> eyre::Result<Vec<u8>> { serde_json::to_vec(&self).map_err(Into::into) } fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { serde_json::from_slice(&response).map_err(Into::into) } }

impl Method<Starknet> for ProposalRequest {
    const METHOD: &'static str = "proposal"; type Returns = Option<Proposal>;
    fn encode(self) -> eyre::Result<Vec<u8>> { let starknet_id: StarknetProposalId = self.proposal_id.into(); let mut call_data = CallData::default(); starknet_id.encode(&mut call_data)?; Ok(call_data.0) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() { return Ok(None); }
        if response.len() % 32 != 0 { return Err(eyre!("Invalid response length: {} bytes is not a multiple of 32", response.len())); }
        let mut felts = Vec::new(); let chunks = response.chunks_exact(32); if !chunks.remainder().is_empty() { return Err(eyre!("Response length is not a multiple of 32 bytes")); }
        for chunk in chunks { let chunk_array: [u8; 32] = chunk.try_into().map_err(|e| eyre!("Failed to convert chunk to array: {}", e))?; felts.push(Felt::from_bytes_be(&chunk_array)); }
        if felts.is_empty() { return Ok(None); }
        match felts[0].to_bytes_be()[31] { 0 => Ok(None), 1 => { let proposal = StarknetProposal::decode(&felts[1..]).map_err(|e| eyre!("Failed to decode proposal: {:?}", e))?; Ok(Some(proposal.into())) } v => Err(eyre!("Invalid option discriminant: {}", v)), }
    }
}

impl Method<Icp> for ProposalRequest { const METHOD: &'static str = "proposals"; type Returns = Option<Proposal>; fn encode(self) -> eyre::Result<Vec<u8>> { let payload = ICRepr::new(*self.proposal_id); Encode!(&payload).map_err(Into::into) } fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let decoded = Decode!(&response, Option<ICProposal>)?; Ok(decoded.map(Into::into)) } }

impl Method<Stellar> for ProposalRequest {
    type Returns = Option<Proposal>; const METHOD: &'static str = "proposal";
    fn encode(self) -> eyre::Result<Vec<u8>> { let env = Env::default(); let proposal_id_raw: [u8; 32] = self.proposal_id.rt().map_err(|e| eyre!("cannot convert proposal id to raw bytes: {}", e))?; let proposal_id_val: BytesN<32> = proposal_id_raw.into_val(&env); let args = (proposal_id_val,); let xdr = args.to_xdr(&env); Ok(xdr.to_alloc_vec()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let cursor = Cursor::new(response); let mut limited = Limited::new(cursor, Limits::none()); let sc_val = ScVal::read_xdr(&mut limited).map_err(|e| eyre!("Failed to read XDR: {}", e))?; if sc_val == ScVal::Void { return Ok(None); } let env = Env::default(); let proposal_val = Val::try_from_val(&env, &sc_val).map_err(|e| eyre!("Failed to convert to proposal: {:?}", e))?; let proposal = StellarProposal::try_from_val(&env, &proposal_val).map_err(|e| eyre!("Failed to convert to proposal: {:?}", e))?; Ok(Some(Proposal::from(proposal))) }
}

impl Method<Ethereum> for ProposalRequest { type Returns = Option<Proposal>; const METHOD: &'static str = "getProposal(bytes32)"; fn encode(self) -> eyre::Result<Vec<u8>> { let proposal_id: [u8; 32] = self.proposal_id.rt().map_err(|e| eyre!("Failed to convert proposal_id: {}", e))?; Ok(proposal_id.abi_encode()) } fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { if response.is_empty() || response.iter().all(|&b| b == 0) { return Ok(None); } let sol_proposal: SolProposal = SolValue::abi_decode(&response, false)?; sol_proposal.try_into().map(Some) } }

// Inlined from proposals.rs
#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct ProposalsRequest { pub(super) offset: usize, pub(super) length: usize }

impl Method<Near> for ProposalsRequest { const METHOD: &'static str = "proposals"; type Returns = Vec<Proposal>; fn encode(self) -> eyre::Result<Vec<u8>> { serde_json::to_vec(&self).map_err(Into::into) } fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { serde_json::from_slice(&response).map_err(Into::into) } }

impl Method<Starknet> for ProposalsRequest {
    const METHOD: &'static str = "proposals"; type Returns = Vec<Proposal>;
    fn encode(self) -> eyre::Result<Vec<u8>> { let req = StarknetProposalsRequest { offset: Felt::from(self.offset as u64), length: Felt::from(self.length as u64) }; let mut call_data = CallData::default(); req.encode(&mut call_data)?; Ok(call_data.0) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() { return Ok(Vec::new()); }
        if response.len() % 32 != 0 { return Err(eyre!("Invalid response length: {} bytes is not a multiple of 32", response.len())); }
        let mut felts = Vec::new(); let chunks = response.chunks_exact(32); if !chunks.remainder().is_empty() { return Err(eyre!("Response length is not a multiple of 32 bytes")); }
        for chunk in chunks { let chunk_array: [u8; 32] = chunk.try_into().map_err(|e| eyre!("Failed to convert chunk to array: {}", e))?; felts.push(Felt::from_bytes_be(&chunk_array)); }
        if felts.is_empty() { return Ok(Vec::new()); }
        let proposals = StarknetProposals::decode(&felts).map_err(|e| eyre!("Failed to decode proposals: {:?}", e))?; Ok(proposals.into())
    }
}

impl Method<Icp> for ProposalsRequest { const METHOD: &'static str = "proposals"; type Returns = Vec<Proposal>; fn encode(self) -> eyre::Result<Vec<u8>> { Encode!(&self.offset, &self.length).map_err(Into::into) } fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let proposals = Decode!(&response, Vec<ICProposal>)?; Ok(proposals.into_iter().map(|id| id.into()).collect()) } }

impl Method<Stellar> for ProposalsRequest {
    type Returns = Vec<Proposal>; const METHOD: &'static str = "proposals";
    fn encode(self) -> eyre::Result<Vec<u8>> { let env = Env::default(); let offset_val: u32 = self.offset as u32; let length_val: u32 = self.length as u32; let args = (offset_val, length_val); let xdr = args.to_xdr(&env); Ok(xdr.to_alloc_vec()) }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let cursor = Cursor::new(response); let mut limited = Limited::new(cursor, Limits::none()); let sc_val = ScVal::read_xdr(&mut limited).map_err(|e| eyre!("Failed to read XDR: {}", e))?; let env = Env::default(); let proposals: soroban_sdk::Vec<StellarProposal> = sc_val.try_into_val(&env).map_err(|e| eyre!("Failed to convert to proposals: {:?}", e))?; Ok(proposals.iter().map(|p| Proposal::from(p.clone())).collect()) }
}

impl Method<Ethereum> for ProposalsRequest { type Returns = Vec<Proposal>; const METHOD: &'static str = "getProposals(uint32,uint32)"; fn encode(self) -> eyre::Result<Vec<u8>> { let offset = u32::try_from(self.offset).map_err(|e| eyre!("Offset too large for u32: {}", e))?; let length = u32::try_from(self.length).map_err(|e| eyre!("Limit too large for u32: {}", e))?; Ok((offset, length).abi_encode()) } fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> { let proposals: Vec<SolProposal> = SolValue::abi_decode(&response, false)?; proposals.into_iter().map(TryInto::try_into).collect() } }
