use std::io::Cursor;

use alloy_sol_types::SolValue;
use candid::{Decode, Encode};
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{Env, TryIntoVal};
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode};
use starknet_crypto::Felt;

use crate::client::env::proxy::ethereum::SolProposal;
use crate::client::env::proxy::starknet::{CallData, StarknetProposals, StarknetProposalsRequest};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::ICProposal;
use crate::stellar::StellarProposal;
use crate::Proposal;

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct ProposalsRequest {
    pub(super) offset: usize,
    pub(super) length: usize,
}

impl Method<Near> for ProposalsRequest {
    const METHOD: &'static str = "proposals";

    type Returns = Vec<Proposal>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for ProposalsRequest {
    const METHOD: &'static str = "proposals";

    type Returns = Vec<Proposal>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let req = StarknetProposalsRequest {
            offset: Felt::from(self.offset as u64),
            length: Felt::from(self.length as u64),
        };
        let mut call_data = CallData::default();
        req.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(Vec::new());
        }

        if response.len() % 32 != 0 {
            return Err(eyre::eyre!(
                "Invalid response length: {} bytes is not a multiple of 32",
                response.len()
            ));
        }

        // Convert bytes to Felts
        let mut felts = Vec::new();
        let chunks = response.chunks_exact(32);

        // Verify no remainder
        if !chunks.remainder().is_empty() {
            return Err(eyre::eyre!("Response length is not a multiple of 32 bytes"));
        }

        for chunk in chunks {
            let chunk_array: [u8; 32] = chunk
                .try_into()
                .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
            felts.push(Felt::from_bytes_be(&chunk_array));
        }

        if felts.is_empty() {
            return Ok(Vec::new());
        }

        // Decode the array
        let proposals = StarknetProposals::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode proposals: {:?}", e))?;

        Ok(proposals.into())
    }
}

impl Method<Icp> for ProposalsRequest {
    const METHOD: &'static str = "proposals";

    type Returns = Vec<Proposal>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        Encode!(&self.offset, &self.length).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let proposals = Decode!(&response, Vec<ICProposal>)?;

        let proposals = proposals.into_iter().map(|id| id.into()).collect();

        Ok(proposals)
    }
}

impl Method<Stellar> for ProposalsRequest {
    type Returns = Vec<Proposal>;

    const METHOD: &'static str = "proposals";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let offset_val: u32 = self.offset as u32;
        let length_val: u32 = self.length as u32;

        let args = (offset_val, length_val);
        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let env = Env::default();
        let proposals: soroban_sdk::Vec<StellarProposal> = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert to proposals: {:?}", e))?;

        // Convert each StellarProposal to our domain Proposal type using the From impl
        Ok(proposals
            .iter()
            .map(|p| Proposal::from(p.clone()))
            .collect())
    }
}

impl Method<Ethereum> for ProposalsRequest {
    type Returns = Vec<Proposal>;

    const METHOD: &'static str = "getProposals(uint32,uint32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let offset = u32::try_from(self.offset)
            .map_err(|e| eyre::eyre!("Offset too large for u32: {}", e))?;
        let length = u32::try_from(self.length)
            .map_err(|e| eyre::eyre!("Limit too large for u32: {}", e))?;

        Ok((offset, length).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let proposals: Vec<SolProposal> = SolValue::abi_decode(&response, false)?;

        proposals.into_iter().map(TryInto::try_into).collect()
    }
}
