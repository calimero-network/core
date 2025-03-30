use std::io::Cursor;

use alloy_sol_types::SolValue;
use candid::{Decode, Encode};
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{Bytes, Env, TryIntoVal};
use starknet::core::codec::Encode as StarknetEncode;
use starknet_crypto::Felt;

use crate::client::env::proxy::starknet::{CallData, ContextVariableKey};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::repr::ICRepr;

#[derive(Clone, Debug, Serialize)]
pub(super) struct ContextVariableRequest {
    pub(super) key: Vec<u8>,
}

impl Method<Near> for ContextVariableRequest {
    const METHOD: &'static str = "get_context_value";

    type Returns = Vec<u8>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for ContextVariableRequest {
    const METHOD: &'static str = "get_context_value";

    type Returns = Vec<u8>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();
        let key: ContextVariableKey = self.key.into();
        key.encode(&mut call_data)?;

        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(vec![]);
        }

        let chunks = response.chunks_exact(32);
        let felts: Vec<Felt> = chunks
            .map(|chunk| {
                let chunk_array: [u8; 32] = chunk
                    .try_into()
                    .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
                Ok(Felt::from_bytes_be(&chunk_array))
            })
            .collect::<eyre::Result<Vec<Felt>>>()?;

        if felts.is_empty() {
            return Ok(vec![]);
        }

        // First felt is the discriminant (0 for None, 1 for Some)
        match felts[0] {
            f if f == Felt::ZERO => {
                println!(
                    "First few bytes after discriminant: {:?}",
                    &response[32..40]
                );

                // Skip first 64 bytes (discriminant + length) and filter nulls
                Ok(response[64..]
                    .iter()
                    .filter(|&&b| b != 0)
                    .copied()
                    .collect())
            }
            v => Err(eyre::eyre!("Invalid option discriminant: {}", v)),
        }
    }
}

impl Method<Icp> for ContextVariableRequest {
    const METHOD: &'static str = "get_context_value";

    type Returns = Vec<u8>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Convert the key to ICRepr
        let payload = ICRepr::new(self.key);
        // Use candid's Encode macro to serialize the data
        Encode!(&payload).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Use candid's Decode macro to deserialize the response
        // The response will be an Option<Vec<u8>>
        let decoded = Decode!(&response, Vec<u8>)?;
        Ok(decoded)
    }
}

impl Method<Stellar> for ContextVariableRequest {
    type Returns = Vec<u8>;

    const METHOD: &'static str = "get_context_value";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();

        let key_val: Bytes = Bytes::from_slice(&env, &self.key);

        let args = (key_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        // Handle None case
        if sc_val == ScVal::Void {
            return Ok(Vec::new()); // Return empty vec if no value found
        }

        let env = Env::default();
        let value: Bytes = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert to Bytes: {:?}", e))?;

        Ok(value.to_alloc_vec())
    }
}

impl Method<Ethereum> for ContextVariableRequest {
    type Returns = Vec<u8>;

    const METHOD: &'static str = "getContextValue(bytes)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        Ok(self.key.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let context_value: Self::Returns = SolValue::abi_decode(&response, false)?;
        Ok(context_value)
    }
}
