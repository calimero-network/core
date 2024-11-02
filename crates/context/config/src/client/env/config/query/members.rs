use serde::{Deserialize, Serialize};

use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::Method;
use crate::repr::Repr;
use crate::types::{ContextId, ContextIdentity};

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Members {
    pub(crate) context_id: Repr<ContextId>,
    pub(crate) offset: usize,
    pub(crate) length: usize,
}

impl Method<Members> for Near {
    const METHOD: &'static str = "members";

    type Returns = Vec<Repr<ContextIdentity>>;

    fn encode(params: &Members) -> eyre::Result<Vec<u8>> {
        let encoded_body = serde_json::to_vec(&params)?;
        Ok(encoded_body)
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        let decoded_body = serde_json::from_slice(response)?;
        Ok(decoded_body)
    }
}

impl Method<Members> for Starknet {
    type Returns = Vec<Repr<ContextIdentity>>;

    const METHOD: &'static str = "members";

    fn encode(params: &Members) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
