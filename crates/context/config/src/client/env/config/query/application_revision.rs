use serde::{Deserialize, Serialize};

use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::Method;
use crate::repr::Repr;
use crate::types::ContextId;

pub type Revision = u64;

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct ApplicationRevision {
    pub(crate) context_id: Repr<ContextId>,
}

impl Method<ApplicationRevision> for Near {
    const METHOD: &'static str = "application_revision";

    type Returns = Revision;

    fn encode(params: &ApplicationRevision) -> eyre::Result<Vec<u8>> {
        let encoded_body = serde_json::to_vec(&params)?;
        Ok(encoded_body)
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        let decoded_body = serde_json::from_slice(response)?;
        Ok(decoded_body)
    }
}

impl Method<ApplicationRevision> for Starknet {
    type Returns = Revision;

    const METHOD: &'static str = "application_revision";

    fn encode(params: &ApplicationRevision) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
