use candid::{Decode, Encode};
use serde::{Deserialize, Serialize};
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode};
use starknet::core::types::Felt;

use crate::client::env::proxy::starknet::CallData;
use crate::client::env::proxy::types::starknet::{
    StarknetProposalId, StarknetProposalWithApprovals,
};
use crate::client::env::Method;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::repr::ICRepr;
use crate::icp::ICProposalWithApprovals;
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
        todo!()
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
