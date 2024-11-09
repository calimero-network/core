use serde::Serialize;

use super::ProposalId;
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::Proposal;

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct ProposalRequest {
    pub(super) offset: usize,
    pub(super) length: usize,
}

impl Method<Near> for ProposalRequest {
    const METHOD: &'static str = "proposals";

    type Returns = Vec<(ProposalId, Proposal)>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let proposals: Vec<(ProposalId, Proposal)> = serde_json::from_slice(&response)?;
        Ok(proposals)
    }
}

impl Method<Starknet> for ProposalRequest {
    const METHOD: &'static str = "proposals";

    type Returns = Vec<(ProposalId, Proposal)>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
