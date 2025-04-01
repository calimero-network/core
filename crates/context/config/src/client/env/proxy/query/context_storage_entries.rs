use std::io::Cursor;

use alloy::dyn_abi::{DynSolType, DynSolValue};
use alloy_sol_types::SolValue;
use candid::{Decode, Encode};
use eyre::eyre;
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{Bytes, Env, TryIntoVal};
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode};
use starknet_crypto::Felt;

use crate::client::env::proxy::starknet::{
    CallData, ContextStorageEntriesResponse, StarknetContextStorageEntriesRequest,
};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::types::ContextStorageEntry;

#[derive(Clone, Debug, Serialize)]
pub(super) struct ContextStorageEntriesRequest {
    pub(super) offset: usize,
    pub(super) limit: usize,
}

impl Method<Near> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";

    type Returns = Vec<ContextStorageEntry>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Decode the response as Vec of tuples with boxed slices
        let entries: Vec<(Box<[u8]>, Box<[u8]>)> = serde_json::from_slice(&response)
            .map_err(|e| eyre!("Failed to decode response: {}", e))?;

        // Convert to ContextStorageEntry
        Ok(entries
            .into_iter()
            .map(|(key, value)| ContextStorageEntry {
                key: key.into(),
                value: value.into(),
            })
            .collect())
    }
}

impl Method<Starknet> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";

    type Returns = Vec<ContextStorageEntry>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let req = StarknetContextStorageEntriesRequest {
            offset: Felt::from(self.offset as u64),
            length: Felt::from(self.limit as u64),
        };
        let mut call_data = CallData::default();
        req.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(vec![]);
        }

        // Convert bytes to Felts
        let chunks = response.chunks_exact(32);
        let felts: Vec<Felt> = chunks
            .map(|chunk| {
                let chunk_array: [u8; 32] = chunk
                    .try_into()
                    .map_err(|e| eyre!("Failed to convert chunk to array: {}", e))?;
                Ok(Felt::from_bytes_be(&chunk_array))
            })
            .collect::<eyre::Result<Vec<Felt>>>()?;

        let response = ContextStorageEntriesResponse::decode_iter(&mut felts.iter())?;

        Ok(response.entries.into_iter().map(Into::into).collect())
    }
}

impl Method<Icp> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";

    type Returns = Vec<ContextStorageEntry>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Encode offset and limit using Candid
        Encode!(&self.offset, &self.limit).map_err(|e| eyre!("Failed to encode request: {}", e))
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Decode the response as Vec of tuples
        let entries: Vec<(Vec<u8>, Vec<u8>)> = Decode!(&response, Vec<(Vec<u8>, Vec<u8>)>)
            .map_err(|e| eyre!("Failed to decode response: {}", e))?;

        // Convert to ContextStorageEntry
        Ok(entries
            .into_iter()
            .map(|(key, value)| ContextStorageEntry { key, value })
            .collect())
    }
}

impl Method<Stellar> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";

    type Returns = Vec<ContextStorageEntry>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let offset_val: u32 = self.offset as u32;
        let limit_val: u32 = self.limit as u32;

        let args = (offset_val, limit_val);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let env = Env::default();
        let entries: soroban_sdk::Vec<(Bytes, Bytes)> = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert to entries: {:?}", e))?;

        // Convert to Vec of ContextStorageEntry
        let result = entries
            .iter()
            .map(|(key, value)| ContextStorageEntry {
                key: key.to_alloc_vec(),
                value: value.to_alloc_vec(),
            })
            .collect();

        Ok(result)
    }
}

impl Method<Ethereum> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "contextStorageEntries(uint32,uint32)";

    type Returns = Vec<ContextStorageEntry>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let offset = u32::try_from(self.offset)
            .map_err(|e| eyre::eyre!("Offset too large for u32: {}", e))?;
        let limit =
            u32::try_from(self.limit).map_err(|e| eyre::eyre!("Limit too large for u32: {}", e))?;
        Ok((offset, limit).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Define the struct type as a tuple
        let struct_type = "tuple(bytes,bytes)[]".parse::<DynSolType>()?;
        // Decode using dynamic ABI decoder
        let decoded = struct_type.abi_decode(&response)?;
        // Convert the decoded value to our type
        let DynSolValue::Array(entries) = decoded else {
            return Err(eyre!("Expected array"));
        };

        Ok(entries
            .into_iter()
            .map(|entry| {
                let DynSolValue::Tuple(fields) = entry else {
                    return Err(eyre!("Expected tuple"));
                };

                let all_bytes = fields[1]
                    .as_bytes()
                    .ok_or_else(|| eyre!("Failed to get bytes from field"))?;

                // Get key
                let key_len = all_bytes[31] as usize;
                let key = all_bytes[32..32 + key_len].to_vec();

                // Get value
                #[allow(clippy::integer_division, reason = "Need this for 32-byte alignment")]
                let value_offset = 32 + ((key_len + 31) / 32) * 32;
                let value_len = all_bytes[value_offset + 31] as usize;
                let value = all_bytes[value_offset + 32..value_offset + 32 + value_len].to_vec();

                Ok(ContextStorageEntry { key, value })
            })
            .collect::<Result<Vec<_>, _>>()?)
    }
}
