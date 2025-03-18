#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
use std::io::Cursor;

use alloy_sol_types::SolValue;
use candid::Decode;
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{BytesN, Env, IntoVal};
use starknet::core::codec::Encode as StarknetEncode;

use crate::client::env::config::types::starknet::{CallData, FeltPair};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::repr::{Repr, ReprTransmute};
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
        let mut encoded = Vec::new();

        let context_raw: [u8; 32] = self
            .context_id
            .rt()
            .map_err(|e| eyre::eyre!("cannot convert context id to raw bytes: {}", e))?;
        encoded.extend_from_slice(&context_raw);

        let member_raw: [u8; 32] = self
            .identity
            .rt()
            .map_err(|e| eyre::eyre!("cannot convert identity to raw bytes: {}", e))?;
        encoded.extend_from_slice(&member_raw);

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let value = Decode!(&response, Self::Returns)?;
        Ok(value)
    }
}

impl Method<Stellar> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "has_member";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id_bytes: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id: BytesN<32> = context_id_bytes.into_val(&env);
        let identity_bytes: [u8; 32] = self.identity.rt().expect("infallible conversion");
        let identity: BytesN<32> = identity_bytes.into_val(&env);

        let args = (context_id, identity);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let result: bool = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to bool: {:?}", e))?;

        Ok(result)
    }
}

impl Method<Ethereum> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "hasMember(bytes32,bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let identity_bytes: [u8; 32] = self.identity.rt().expect("infallible conversion");

        Ok((context_id, identity_bytes).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let result: bool = SolValue::abi_decode(&response, false)?;
        Ok(result)
    }
}
