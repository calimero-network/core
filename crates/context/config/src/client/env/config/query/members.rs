#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
use core::mem;
use std::io::Cursor;

use alloy::primitives::B256;
use alloy_sol_types::SolValue;
use candid::{Decode, Encode};
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{BytesN, Env, IntoVal, TryIntoVal};
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode};
use starknet_crypto::Felt;

use crate::client::env::config::types::starknet::{
    CallData, StarknetMembers, StarknetMembersRequest,
};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::repr::ICRepr;
use crate::repr::{Repr, ReprTransmute};
use crate::types::{ContextId, ContextIdentity};

#[derive(Copy, Clone, Debug, Serialize)]
pub(crate) struct MembersRequest {
    pub(crate) context_id: Repr<ContextId>,
    pub(crate) offset: usize,
    pub(crate) length: usize,
}

impl Method<Near> for MembersRequest {
    const METHOD: &'static str = "members";

    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let members: Vec<Repr<ContextIdentity>> = serde_json::from_slice(&response)?;

        // safety: `Repr<T>` is a transparent wrapper around `T`
        #[expect(
            clippy::transmute_undefined_repr,
            reason = "Repr<T> is a transparent wrapper around T"
        )]
        let members =
            unsafe { mem::transmute::<Vec<Repr<ContextIdentity>>, Vec<ContextIdentity>>(members) };

        Ok(members)
    }
}

impl Method<Starknet> for MembersRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "members";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let req: StarknetMembersRequest = self.into();
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

        // Check if it's a None response (single zero Felt)
        if felts.len() == 1 && felts[0] == Felt::ZERO {
            return Ok(Vec::new());
        }

        // Decode directly from the felts slice - the Decode trait will handle the array structure
        let members = StarknetMembers::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode members: {:?}", e))?;

        Ok(members.into())
    }
}

impl Method<Icp> for MembersRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "members";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);

        Encode!(&context_id, &self.offset, &self.length).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let members = Decode!(&response, Vec<ICRepr<ContextIdentity>>)?;

        // safety: `ICRepr<T>` is a transparent wrapper around `T`
        #[expect(
            clippy::transmute_undefined_repr,
            reason = "ICRepr<T> is a transparent wrapper around T"
        )]
        let members = unsafe {
            mem::transmute::<Vec<ICRepr<ContextIdentity>>, Vec<ContextIdentity>>(members)
        };

        Ok(members)
    }
}

impl Method<Stellar> for MembersRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "members";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: BytesN<32> = context_id.into_val(&env);

        let offset_val: u32 = self.offset as u32;
        let length_val: u32 = self.length as u32;

        let args = (context_id_val, offset_val, length_val);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let env = Env::default();
        let members: soroban_sdk::Vec<BytesN<32>> = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert to Vec<BytesN<32>>: {:?}", e))?;

        Ok(members
            .iter()
            .map(|id| id.to_array().rt().expect("infallible conversion"))
            .collect())
    }
}

impl Method<Ethereum> for MembersRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "members(bytes32,uint256,uint256)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        let offset_val: u64 = self.offset as u64;
        let length_val: u64 = self.length as u64;

        Ok((context_id, offset_val, length_val).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Decode Vec<B256> directly from response
        let decoded: Vec<B256> = SolValue::abi_decode(&response, false)?;

        // Convert each B256 to ContextIdentity
        Ok(decoded
            .into_iter()
            .map(|b| b.rt().expect("infallible conversion"))
            .collect())
    }
}
