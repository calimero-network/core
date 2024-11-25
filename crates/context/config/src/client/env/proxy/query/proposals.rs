use serde::Serialize;
use starknet::core::codec::Decode;
use starknet_crypto::Felt;

use crate::client::env::proxy::starknet::StarknetProposals;
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
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
        let mut bytes = Vec::with_capacity(64); // 2 * 32 bytes for two u32 parameters

        // First parameter (offset): pad to 32 bytes
        bytes.extend_from_slice(&[0; 28]); // 28 bytes of zeros
        bytes.extend_from_slice(&(self.offset as u32).to_be_bytes()); // 4 bytes of actual value

        // Second parameter (length): pad to 32 bytes
        bytes.extend_from_slice(&[0; 28]); // 28 bytes of zeros
        bytes.extend_from_slice(&(self.length as u32).to_be_bytes()); // 4 bytes of actual value

        Ok(bytes)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Convert bytes to Felts
        let mut felts = Vec::new();
        for chunk in response.chunks(32) {
            if chunk.len() == 32 {
                felts.push(Felt::from_bytes_be(chunk.try_into().unwrap()));
            }
        }

        // Skip version felt and decode the array
        let proposals = StarknetProposals::decode(&felts)
            .map_err(|e| {
                println!("Raw felts: {:?}", felts);
                eyre::eyre!("Failed to decode proposals: {:?}", e)
            })?;

        Ok(proposals.into())
    }
}
