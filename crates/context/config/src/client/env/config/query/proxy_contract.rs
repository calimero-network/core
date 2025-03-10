#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
use std::io::Cursor;

use alloy::primitives::B256;
use alloy_sol_types::abi::{encode, Token};
use alloy_sol_types::SolValue;
use candid::{Decode, Encode, Principal};
use hex;
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{Address, BytesN, Env, IntoVal, TryFromVal};
use starknet::core::codec::Encode as StarknetEncode;
use starknet_crypto::Felt;

use crate::client::env::config::types::starknet::{CallData, FeltPair};
use crate::client::env::Method;
use crate::client::protocol::evm::Evm;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::repr::ICRepr;
use crate::repr::{Repr, ReprTransmute};
use crate::types::ContextId;

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct ProxyContractRequest {
    pub(super) context_id: Repr<ContextId>,
}

impl Method<Near> for ProxyContractRequest {
    const METHOD: &'static str = "proxy_contract";

    type Returns = String;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for ProxyContractRequest {
    const METHOD: &'static str = "proxy_contract";

    type Returns = String;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();
        let felt_pair: FeltPair = self.context_id.into();
        felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("No proxy contract found"));
        }

        // Check if it's a None response (single zero Felt)
        if response.iter().all(|&x| x == 0) {
            return Err(eyre::eyre!("No proxy contract found"));
        }

        // Parse bytes as Felt
        let felt = Felt::from_bytes_be_slice(&response);

        // Format felt as hex string with 0x prefix
        Ok(format!("0x{:x}", felt))
    }
}

impl Method<Icp> for ProxyContractRequest {
    const METHOD: &'static str = "proxy_contract";

    type Returns = String;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);
        Encode!(&context_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let value: Principal = Decode!(&response, Principal)?;
        let value_as_string = value.to_text();
        Ok(value_as_string)
    }
}

impl Method<Stellar> for ProxyContractRequest {
    type Returns = String;

    const METHOD: &'static str = "proxy_contract";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_val: BytesN<32> = context_raw.into_val(&env);

        let args = (context_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let env = Env::default();
        let address = Address::try_from_val(&env, &sc_val)
            .map_err(|e| eyre::eyre!("Failed to convert to address: {:?}", e))?;

        Ok(address.to_string().to_string())
    }
}

impl Method<Evm> for ProxyContractRequest {
    type Returns = String;

    const METHOD: &'static str = "proxyContract(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_bytes = B256::from_slice(&context_id);

        let encoded_context_id = SolValue::abi_encode(&context_id_bytes);
        Ok(encoded_context_id.to_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Check if the response is empty
        if response.is_empty() {
            return Err(eyre::eyre!("Empty response from contract. The context might not exist or the proxy contract address is not set."));
        }

        // Convert the bytes to a string (since we know it's a UTF-8 string like "0x000000...")
        let response_str = String::from_utf8(response)
            .map_err(|e| eyre::eyre!("Failed to convert response bytes to string: {}", e))?;

        // Remove the "0x" prefix
        let hex_str = response_str.trim_start_matches("0x");

        // Decode the hex string to get the actual binary data
        let decoded_bytes =
            hex::decode(hex_str).map_err(|e| eyre::eyre!("Failed to decode hex string: {}", e))?;

        // For an address, we expect exactly 32 bytes (padded address)
        if decoded_bytes.len() != 32 {
            return Err(eyre::eyre!(
                "Expected 32 bytes for address after decoding, got {}: {:?}",
                decoded_bytes.len(),
                decoded_bytes
            ));
        }

        // Extract the address (last 20 bytes of the 32-byte word)
        let address_bytes = &decoded_bytes[12..32];

        // Check if the address is zero
        if address_bytes.iter().all(|&b| b == 0) {
            return Err(eyre::eyre!(
                "Proxy contract address is zero. This could mean the proxy deployment failed."
            ));
        }

        // Convert to hex string with 0x prefix
        let address = format!("0x{}", hex::encode(address_bytes));

        println!("Extracted address: {}", address);
        Ok(address)
    }
}
