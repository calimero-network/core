use std::io::Cursor;

use alloy_sol_types::SolValue;
use candid::{CandidType, Decode, Encode};
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal};

use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;

#[derive(Copy, Clone, Debug, Serialize, CandidType)]
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
            return Err(eyre::eyre!(
                "Invalid response format: non-zero bytes in prefix"
            ));
        }

        // Take the last two bytes for u16
        let value = u16::from_be_bytes([response[30], response[31]]);

        Ok(value)
    }
}

impl Method<Icp> for ActiveProposalRequest {
    const METHOD: &'static str = "get_active_proposals_limit";

    type Returns = u16;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        Encode!(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let value = Decode!(&response, u32)?;
        Ok(value as u16)
    }
}

impl Method<Stellar> for ActiveProposalRequest {
    type Returns = u16;

    const METHOD: &'static str = "get_active_proposals_limit";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let active_proposals_limit: u32 = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to u64: {:?}", e))?;

        Ok(active_proposals_limit as u16)
    }
}

impl Method<Ethereum> for ActiveProposalRequest {
    type Returns = u16;

    const METHOD: &'static str = "getActiveProposalsLimit()";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        Ok(().abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let active_proposals_limit: Self::Returns = SolValue::abi_decode(&response, false)?;
        Ok(active_proposals_limit)
    }
}
