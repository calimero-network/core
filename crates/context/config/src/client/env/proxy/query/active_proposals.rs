use serde::Serialize;

use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct ActiveProposalRequest;

impl Method<Near> for ActiveProposalRequest {
    const METHOD: &'static str = "get_active_proposals_limit";

    type Returns = u16;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for ActiveProposalRequest {
    const METHOD: &'static str = "get_active_proposals_limit";

    type Returns = u16;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
