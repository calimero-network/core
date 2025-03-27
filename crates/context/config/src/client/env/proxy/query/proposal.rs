use std::io::Cursor;

use alloy_sol_types::SolValue;
use candid::{Decode, Encode};
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{BytesN, Env, IntoVal, TryFromVal, Val};
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode};
use starknet_crypto::Felt;

use crate::client::env::proxy::ethereum::SolProposal;
use crate::client::env::proxy::starknet::CallData;
use crate::client::env::proxy::types::starknet::{StarknetProposal, StarknetProposalId};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::repr::ICRepr;
use crate::icp::ICProposal;
use crate::repr::{Repr, ReprTransmute};
use crate::stellar::StellarProposal;
use crate::types::ProposalId;
use crate::Proposal;

#[derive(Clone, Debug, Serialize)]
pub(super) struct ProposalRequest {
    pub(super) proposal_id: Repr<ProposalId>,
}

impl Method<Near> for ProposalRequest {
    const METHOD: &'static str = "proposal";

    type Returns = Option<Proposal>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
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

        if felts.is_empty() {
            return Ok(None);
        }

        // First felt should be 1 for Some, 0 for None
        match felts[0].to_bytes_be()[31] {
            0 => Ok(None),
            1 => {
                // Decode the proposal starting from index 1
                let proposal = StarknetProposal::decode(&felts[1..])
                    .map_err(|e| eyre::eyre!("Failed to decode proposal: {:?}", e))?;
                Ok(Some(proposal.into()))
            }
            v => Err(eyre::eyre!("Invalid option discriminant: {}", v)),
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

        // Handle None case
        if sc_val == ScVal::Void {
            return Ok(None);
        }

        let env = Env::default();
        let proposal_val = Val::try_from_val(&env, &sc_val)
            .map_err(|e| eyre::eyre!("Failed to convert to proposal: {:?}", e))?;
        let proposal = StellarProposal::try_from_val(&env, &proposal_val)
            .map_err(|e| eyre::eyre!("Failed to convert to proposal: {:?}", e))?;

        // Convert StellarProposal to Proposal using our From impl
        Ok(Some(Proposal::from(proposal)))
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
        // If response is empty or all zeros, return None
        if response.is_empty() || response.iter().all(|&b| b == 0) {
            return Ok(None);
        }

        // Decode the SolProposal from the response
        let sol_proposal: SolProposal = SolValue::abi_decode(&response, false)?;

        // Convert to our Proposal type using the From implementation we created
        sol_proposal.try_into().map(Some)
    }
}
