use serde::Serialize;
use starknet::core::codec::{Decode, Encode};
use starknet_crypto::Felt;

use super::ProposalId;
use crate::client::env::proxy::starknet::CallData;
use crate::client::env::proxy::types::starknet::{StarknetProposal, StarknetProposalId};
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::icp::Icp;
use crate::repr::Repr;
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
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}