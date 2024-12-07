use candid::{Decode, Encode};
use serde::Serialize;
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode};
use starknet_crypto::Felt;

use crate::client::env::proxy::starknet::{CallData, StarknetProposals, StarknetProposalsRequest};
use crate::client::env::Method;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::icp::ICProposal;
use crate::Proposal;

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct ProposalsRequest {
    pub(super) offset: usize,
    pub(super) length: usize,
}

impl Method<Near> for ProposalsRequest {
    const METHOD: &'static str = "proposals";

    type Returns = Vec<Proposal>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for ProposalsRequest {
    const METHOD: &'static str = "proposals";

    type Returns = Vec<Proposal>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let req = StarknetProposalsRequest {
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
            return Ok(Vec::new());
        }

        // Decode the array
        let proposals = StarknetProposals::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode proposals: {:?}", e))?;

        Ok(proposals.into())
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

        let proposals = proposals.into_iter().map(|id| id.into()).collect();

        Ok(proposals)
    }
}
