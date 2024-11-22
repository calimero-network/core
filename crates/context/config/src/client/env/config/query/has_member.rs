use serde::Serialize;
use starknet_crypto::Felt;

use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::{Repr, ReprBytes};
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
        let mut result = Vec::new();

        // Encode context_id (2 felts)
        let context_bytes = self.context_id.as_bytes();
        let (context_high, context_low) = context_bytes.split_at(context_bytes.len() / 2);

        // Convert to Felts and add to result
        let context_high_felt = Felt::from_bytes_be_slice(context_high);
        let context_low_felt = Felt::from_bytes_be_slice(context_low);
        result.extend_from_slice(&context_high_felt.to_bytes_be());
        result.extend_from_slice(&context_low_felt.to_bytes_be());

        // Encode member identity (2 felts)
        let identity_bytes = self.identity.as_bytes();
        let (identity_high, identity_low) = identity_bytes.split_at(identity_bytes.len() / 2);

        // Convert to Felts and add to result
        let identity_high_felt = Felt::from_bytes_be_slice(identity_high);
        let identity_low_felt = Felt::from_bytes_be_slice(identity_low);
        result.extend_from_slice(&identity_high_felt.to_bytes_be());
        result.extend_from_slice(&identity_low_felt.to_bytes_be());

        Ok(result)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("Empty response"));
        }

        // Response should be a single felt (32 bytes) representing 0 or 1
        if response.len() != 32 {
            return Err(eyre::eyre!("Invalid response length"));
        }

        // Check the last byte for 0 or 1
        Ok(response[31] == 1)
    }
}
