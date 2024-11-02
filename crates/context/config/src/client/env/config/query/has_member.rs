use serde::Serialize;

use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::Repr;
use crate::types::{ContextId, ContextIdentity};

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct HasMemberRequest {
    pub(super) context_id: Repr<ContextId>,
    pub(super) identity: Repr<ContextIdentity>,
}

impl Method<Near> for HasMemberRequest {
    const METHOD: &'static str = "has_member";

    type Returns = bool;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "has_member";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
