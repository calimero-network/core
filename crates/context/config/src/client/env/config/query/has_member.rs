use serde::Serialize;
use starknet::core::codec::Encode;

use crate::client::env::config::types::starknet::{CallData, FeltPair};
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::icp::Icp;
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
        let mut call_data = CallData::default();

        // Encode context_id
        let context_pair: FeltPair = self.context_id.into();
        context_pair.encode(&mut call_data)?;

        // Encode identity
        let identity_pair: FeltPair = self.identity.into();
        identity_pair.encode(&mut call_data)?;

        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 32 bytes, got {}",
                response.len()
            ));
        }

        // Check if all bytes except the last one are zero
        if !response[..31].iter().all(|&b| b == 0) {
            return Err(eyre::eyre!(
                "Invalid response format: non-zero bytes in prefix"
            ));
        }

        // Check the last byte is either 0 or 1
        match response[31] {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(eyre::eyre!("Invalid boolean value: {}", v)),
        }
    }
}

impl Method<Icp> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "has_member";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
