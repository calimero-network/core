use serde::Serialize;

use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::Repr;
use crate::types::{ContextId, Revision};
use crate::repr::ReprBytes;
use starknet_crypto::Felt;

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
        // Split context_id into high/low parts
        let bytes = self.context_id.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        
        // Convert to Felts
        let high_felt = Felt::from_bytes_be_slice(high_bytes);
        let low_felt = Felt::from_bytes_be_slice(low_bytes);
        
        // Convert both Felts to bytes and concatenate
        let mut result = Vec::new();
        result.extend_from_slice(&high_felt.to_bytes_be());
        result.extend_from_slice(&low_felt.to_bytes_be());

        Ok(result)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("Empty response"));
        }

        // Response should be a single u64 in the last 8 bytes of a felt
        let revision_bytes = &response[24..32];  // Take last 8 bytes
        let revision = u64::from_be_bytes(revision_bytes.try_into()?);
        
        Ok(revision)
    }
}
