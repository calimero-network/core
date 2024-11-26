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
        // No parameters needed for this call
        Ok(Vec::new())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 32 bytes, got {}",
                response.len()
            ));
        }

        // Verify that all bytes except the last two are zero
        if !response[..30].iter().all(|&b| b == 0) {
            return Err(eyre::eyre!("Invalid response format: non-zero bytes in prefix"));
        }

        // Take the last two bytes for u16
        let value = u16::from_be_bytes([response[30], response[31]]);

        Ok(value)
    }
}
