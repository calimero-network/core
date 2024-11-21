use serde::Serialize;
use starknet_crypto::Felt;

use super::ProposalId;
use crate::client::env::proxy::types::starknet::{StarknetProposal, StarknetProposalId};
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::Repr;
use crate::Proposal;
use starknet::core::codec::Decode;

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
        // Convert ProposalId to StarknetProposalId
        let starknet_id: StarknetProposalId = self.proposal_id.into();
        
        // Encode both high and low parts
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&starknet_id.high.to_bytes_be());
        encoded.extend_from_slice(&starknet_id.low.to_bytes_be());
        
        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
         // First check if we got a None response
         if response.is_empty() || response.len() < 32 {
              return Ok(None);
          }

          // Convert bytes to Felts
          let mut felts = Vec::new();
          for chunk in response.chunks(32) {
              if chunk.len() == 32 {
                  felts.push(Felt::from_bytes_be(chunk.try_into().unwrap()));
              }
          }

          // First felt should be 1 for Some, 0 for None
          let is_some = felts[0].to_bytes_be()[31] == 1;
          if !is_some {
              return Ok(None);
          }

          // Decode the proposal starting from index 1
          let proposal = StarknetProposal::decode(&felts[1..])
              .map_err(|e| eyre::eyre!("Failed to decode proposal: {:?}", e))?;
          println!("Decoded proposal: {:?}", proposal);

          
          Ok(Some(proposal.into()))
    }
}
