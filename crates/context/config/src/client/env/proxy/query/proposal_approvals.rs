use std::io::Cursor;

use alloy::primitives::B256;
use alloy_sol_types::SolValue;
use candid::{Decode, Encode};
use eyre::WrapErr;
use serde::{Deserialize, Serialize};
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{BytesN, Env, IntoVal, TryIntoVal, Val};
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode};
use starknet::core::types::Felt;

use crate::client::env::proxy::starknet::CallData;
use crate::client::env::proxy::types::starknet::{
    StarknetProposalId, StarknetProposalWithApprovals,
};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::repr::ICRepr;
use crate::icp::ICProposalWithApprovals;
use crate::repr::ReprTransmute;
use crate::stellar::StellarProposalWithApprovals;
use crate::types::ProposalId;
use crate::{ProposalWithApprovals, Repr};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ProposalApprovalsRequest {
    pub(super) proposal_id: Repr<ProposalId>,
}

impl Method<Near> for ProposalApprovalsRequest {
    const METHOD: &'static str = "get_confirmations_count";

    type Returns = ProposalWithApprovals;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for ProposalApprovalsRequest {
    const METHOD: &'static str = "get_confirmations_count";

    type Returns = ProposalWithApprovals;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Convert ProposalId to StarknetProposalId
        let starknet_id: StarknetProposalId = self.proposal_id.into();

        // Encode both high and low parts
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

        // Convert bytes to Felts
        let mut felts = Vec::new();
        let chunks = response.chunks_exact(32);

        // Verify no remainder
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

        // Handle None case first since it's an Option
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

        // Use the From implementation to convert
        Ok(ProposalWithApprovals::from(stellar_proposal))
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
        let (proposal_id, num_approvals): (B256, u32) = SolValue::abi_decode(&response, false)?;

        Ok(ProposalWithApprovals {
            proposal_id: proposal_id.rt().wrap_err("infallible conversion")?,
            num_approvals: num_approvals as usize,
        })
    }
}
