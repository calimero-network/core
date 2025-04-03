#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
use std::io::Cursor;

use alloy::primitives::Address as AlloyAddress;
use alloy_sol_types::SolValue;
use candid::{Decode, Encode, Principal};
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{Address, BytesN, Env, IntoVal, TryFromVal};
use starknet::core::codec::Encode as StarknetEncode;
use starknet_crypto::Felt;

use crate::client::env::config::types::starknet::{CallData, FeltPair};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
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

impl Method<Ethereum> for ProxyContractRequest {
    type Returns = String;

    const METHOD: &'static str = "proxyContract(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let contract_address: AlloyAddress = SolValue::abi_decode(&response, false)?;

        Ok(contract_address.to_string())
    }
}
