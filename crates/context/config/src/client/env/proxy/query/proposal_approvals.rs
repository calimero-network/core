use serde::Serialize;

use super::ProposalId;
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
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
