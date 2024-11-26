use serde::Serialize;
use starknet::core::codec::Encode;

use crate::client::env::config::types::starknet::{CallData, FeltPair};
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::Repr;
use crate::types::{ContextId, Revision};

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct MembersRevisionRequest {
    pub(super) context_id: Repr<ContextId>,
}

impl Method<Near> for MembersRevisionRequest {
    const METHOD: &'static str = "members_revision";

    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for MembersRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "members_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let felt_pair: FeltPair = self.context_id.into();
        let mut call_data = CallData::default();
        felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 32 bytes, got {}",
                response.len()
            ));
        }

        // Response should be a single u64 in the last 8 bytes of a felt
        // First 24 bytes should be zero
        if !response[..24].iter().all(|&b| b == 0) {
            return Err(eyre::eyre!(
                "Invalid response format: non-zero bytes in prefix"
            ));
        }

        let revision_bytes = &response[24..32];
        let revision = u64::from_be_bytes(revision_bytes.try_into()?);

        Ok(revision)
    }
}
