#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
use std::io::Cursor;

use alloy_sol_types::SolValue;
use candid::{Decode, Encode};
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{BytesN, Env, IntoVal};
use starknet::core::codec::Encode as StarknetEncode;

use crate::client::env::config::types::starknet::{
    CallData, ContextId as StarknetContextId, ContextIdentity as StarknetContextIdentity,
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
pub(super) struct FetchNonceRequest {
    pub(super) context_id: Repr<ContextId>,
    pub(super) member_id: Repr<ContextIdentity>,
}

impl FetchNonceRequest {
    pub const fn new(context_id: ContextId, member_id: ContextIdentity) -> Self {
        Self {
            context_id: Repr::new(context_id),
            member_id: Repr::new(member_id),
        }
    }
}

impl Method<Near> for FetchNonceRequest {
    const METHOD: &'static str = "fetch_nonce";

    type Returns = Option<u64>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();

        // Dereference Repr and encode context_id
        let context_id: StarknetContextId = (*self.context_id).into();
        context_id.encode(&mut call_data)?;

        let member_id: StarknetContextIdentity = (*self.member_id).into();
        member_id.encode(&mut call_data)?;

        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 8 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 8 bytes, got {}",
                response.len()
            ));
        }

        let nonce = u64::from_be_bytes(
            response
                .try_into()
                .map_err(|_| eyre::eyre!("Failed to convert response to u64"))?,
        );

        Ok(Some(nonce))
    }
}

impl Method<Icp> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);
        let member_id = ICRepr::new(*self.member_id);

        // Encode arguments separately
        Encode!(&context_id, &member_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, Option<u64>)?;

        Ok(decoded)
    }
}

impl Method<Stellar> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: BytesN<32> = context_id.into_val(&env);

        let member_id: [u8; 32] = self.member_id.rt().expect("infallible conversion");
        let member_id_val: BytesN<32> = member_id.into_val(&env);

        let args = (context_id_val, member_id_val);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let nonce: u64 = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to u64: {:?}", e))?;

        Ok(Some(nonce))
    }
}

impl Method<Ethereum> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetchNonce(bytes32,bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let member_id: [u8; 32] = self.member_id.rt().expect("infallible conversion");

        Ok((context_id, member_id).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let nonce: u64 = SolValue::abi_decode(&response, false)?;

        Ok(Some(nonce))
    }
}
