use serde::Serialize;

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
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
