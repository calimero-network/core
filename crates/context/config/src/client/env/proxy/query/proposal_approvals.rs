use serde::Serialize;
use starknet::core::codec::Decode;
use starknet::core::types::Felt;

use super::ProposalId;
use crate::client::env::proxy::types::starknet::{
    StarknetProposalId, StarknetProposalWithApprovals,
};
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::Repr;
use crate::ProposalWithApprovals;

#[derive(Clone, Debug, Serialize)]
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
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&starknet_id.0.high.to_bytes_be());
        encoded.extend_from_slice(&starknet_id.0.low.to_bytes_be());

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Convert bytes to Felts
        let mut felts = Vec::new();
        for chunk in response.chunks(32) {
            if chunk.len() == 32 {
                felts.push(Felt::from_bytes_be(chunk.try_into().map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?));
            }
        }

        let approvals = StarknetProposalWithApprovals::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode approvals: {:?}", e))?;

        Ok(approvals.into())
    }
}
