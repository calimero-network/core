use std::mem;

use serde::Serialize;
use starknet::core::codec::Decode;
use starknet::core::types::Felt;

use super::ProposalId;
use crate::client::env::proxy::types::starknet::{StarknetApprovers, StarknetProposalId};
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::Repr;
use crate::repr::ReprBytes;
use crate::types::ContextIdentity;

#[derive(Clone, Debug, Serialize)]
pub(super) struct ProposalApproversRequest {
    pub(super) proposal_id: Repr<ProposalId>,
}

impl Method<Near> for ProposalApproversRequest {
    const METHOD: &'static str = "get_proposal_approvers";

    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let members: Vec<Repr<ContextIdentity>> = serde_json::from_slice(&response)?;

        // safety: `Repr<T>` is a transparent wrapper around `T`
        #[expect(
            clippy::transmute_undefined_repr,
            reason = "Repr<T> is a transparent wrapper around T"
        )]
        let members =
            unsafe { mem::transmute::<Vec<Repr<ContextIdentity>>, Vec<ContextIdentity>>(members) };

        Ok(members)
    }
}

impl Method<Starknet> for ProposalApproversRequest {
    const METHOD: &'static str = "proposal_approvers";

    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Get the full 32 bytes
        let bytes = self.proposal_id.as_bytes();
        
        // Split into high and low parts (16 bytes each)
        let (high_bytes, low_bytes) = bytes.split_at(16);
        
        // Create Felts with proper padding
        let mut high = [0u8; 32];
        let mut low = [0u8; 32];
        high[16..].copy_from_slice(high_bytes);  // Put in last 16 bytes
        low[16..].copy_from_slice(low_bytes);    // Put in last 16 bytes
        
        let starknet_id = StarknetProposalId {
            high: Felt::from_bytes_be(&high),
            low: Felt::from_bytes_be(&low),
        };
        // Encode exactly as in mutate response
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&starknet_id.high.to_bytes_be());
        encoded.extend_from_slice(&starknet_id.low.to_bytes_be());
        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Convert bytes to Felts
        let mut felts = Vec::new();
        for chunk in response.chunks(32) {
            if chunk.len() == 32 {
                felts.push(Felt::from_bytes_be(chunk.try_into().unwrap()));
            }
        }

        let approvers = StarknetApprovers::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode approvers: {:?}", e))?;

        Ok(approvers.into())
    }
}
