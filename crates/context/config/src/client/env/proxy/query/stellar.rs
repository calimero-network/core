#![cfg(feature = "stellar_client")]

//! Stellar specific implementations for context proxy queries.

use std::io::Cursor;

use eyre::eyre;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{Bytes, BytesN, Env, IntoVal, TryFromVal, TryIntoVal, Val};

use super::super::requests::{
    ActiveProposalRequest, ContextStorageEntriesRequest, ContextVariableRequest,
    ProposalApprovalsRequest, ProposalApproversRequest, ProposalRequest, ProposalsRequest,
};
use crate::client::env::Method;
use crate::client::protocol::stellar::Stellar;
use crate::repr::ReprTransmute;
use crate::stellar::{StellarProposal, StellarProposalWithApprovals};
use crate::types::{ContextIdentity, ContextStorageEntry};
use crate::{Proposal, ProposalWithApprovals};

impl Method<Stellar> for ActiveProposalRequest {
    type Returns = u16;
    const METHOD: &'static str = "get_active_proposals_limit";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        Ok(Vec::new())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());
        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;
        let active_proposals_limit: u32 = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to u64: {:?}", e))?;
        Ok(active_proposals_limit as u16)
    }
}

impl Method<Stellar> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";
    type Returns = Vec<ContextStorageEntry>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let offset_val: u32 = self.offset as u32;
        let limit_val: u32 = self.limit as u32;
        let args = (offset_val, limit_val);
        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());
        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;
        let env = Env::default();
        let entries: soroban_sdk::Vec<(Bytes, Bytes)> = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert to entries: {:?}", e))?;
        Ok(entries
            .iter()
            .map(|(key, value)| ContextStorageEntry {
                key: key.to_alloc_vec(),
                value: value.to_alloc_vec(),
            })
            .collect())
    }
}

impl Method<Stellar> for ContextVariableRequest {
    type Returns = Vec<u8>;
    const METHOD: &'static str = "get_context_value";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let key_val: Bytes = Bytes::from_slice(&env, &self.key);
        let args = (key_val,);
        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());
        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;
        if sc_val == ScVal::Void {
            return Ok(Vec::new());
        }
        let env = Env::default();
        let value: Bytes = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert to Bytes: {:?}", e))?;
        Ok(value.to_alloc_vec())
    }
}

impl Method<Stellar> for ProposalApprovalsRequest {
    type Returns = ProposalWithApprovals;
    const METHOD: &'static str = "get_confirmations_count";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let proposal_id_raw: [u8; 32] = self
            .proposal_id
            .rt()
            .map_err(|e| eyre::eyre!("cannot convert proposal id to raw bytes: {}", e))?;
        let proposal_id_val: BytesN<32> = proposal_id_raw.into_val(&env);
        let args = (proposal_id_val,);
        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());
        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;
        if sc_val == ScVal::Void {
            return Err(eyre::eyre!("Proposal not found"));
        }
        let env = Env::default();
        let val: Val = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert ScVal to Val: {:?}", e))?;
        let stellar_proposal: StellarProposalWithApprovals =
            val.try_into_val(&env).map_err(|e| {
                eyre::eyre!("Failed to convert to StellarProposalWithApprovals: {:?}", e)
            })?;
        Ok(ProposalWithApprovals::from(stellar_proposal))
    }
}

impl Method<Stellar> for ProposalApproversRequest {
    type Returns = Vec<ContextIdentity>;
    const METHOD: &'static str = "proposal_approvers";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let proposal_id_raw: [u8; 32] = self
            .proposal_id
            .rt()
            .map_err(|e| eyre::eyre!("Failed to convert proposal_id: {}", e))?;
        let proposal_id_val: BytesN<32> = proposal_id_raw.into_val(&env);
        let args = (proposal_id_val,);
        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());
        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;
        if sc_val == ScVal::Void {
            return Ok(Vec::new());
        }
        let env = Env::default();
        let approvers: soroban_sdk::Vec<BytesN<32>> = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert to approvers: {:?}", e))?;
        approvers
            .iter()
            .map(|bytes| {
                bytes
                    .to_array()
                    .rt()
                    .map_err(|e| eyre::eyre!("Failed to convert bytes to identity: {}", e))
            })
            .collect()
    }
}

impl Method<Stellar> for ProposalRequest {
    type Returns = Option<Proposal>;
    const METHOD: &'static str = "proposal";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let proposal_id_raw: [u8; 32] = self
            .proposal_id
            .rt()
            .map_err(|e| eyre::eyre!("cannot convert proposal id to raw bytes: {}", e))?;
        let proposal_id_val: BytesN<32> = proposal_id_raw.into_val(&env);
        let args = (proposal_id_val,);
        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());
        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;
        if sc_val == ScVal::Void {
            return Ok(None);
        }
        let env = Env::default();
        let proposal_val = Val::try_from_val(&env, &sc_val)
            .map_err(|e| eyre::eyre!("Failed to convert to proposal: {:?}", e))?;
        let proposal = StellarProposal::try_from_val(&env, &proposal_val)
            .map_err(|e| eyre::eyre!("Failed to convert to proposal: {:?}", e))?;
        Ok(Some(Proposal::from(proposal)))
    }
}

impl Method<Stellar> for ProposalsRequest {
    type Returns = Vec<Proposal>;
    const METHOD: &'static str = "proposals";
    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let offset_val: u32 = self.offset as u32;
        let length_val: u32 = self.length as u32;
        let args = (offset_val, length_val);
        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());
        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;
        let env = Env::default();
        let proposals: soroban_sdk::Vec<StellarProposal> = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert to proposals: {:?}", e))?;
        Ok(proposals
            .iter()
            .map(|p| Proposal::from(p.clone()))
            .collect())
    }
}
