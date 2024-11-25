use serde::Serialize;
use starknet_crypto::Felt;

use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::{Repr, ReprBytes};
use crate::types::{ContextId, Revision};

#[derive(Copy, Clone, Debug, Serialize)]
pub struct ApplicationRevisionRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Near> for ApplicationRevisionRequest {
    const METHOD: &'static str = "application_revision";

    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for ApplicationRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "application_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Split context_id into high/low parts
        let bytes = self.context_id.as_bytes();
        let mid_point = bytes.len().checked_div(2).ok_or_else(|| eyre::eyre!("Length should be even"))?;
        let (high_bytes, low_bytes) = bytes.split_at(mid_point);

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
        let revision_bytes = &response[24..32]; // Take last 8 bytes
        let revision = u64::from_be_bytes(revision_bytes.try_into()?);

        Ok(revision)
    }
}
