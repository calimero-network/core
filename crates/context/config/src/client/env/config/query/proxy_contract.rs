use candid::Decode;
use serde::Serialize;
use starknet::core::codec::Encode as StarknetEncode;
use starknet_crypto::Felt;

use crate::client::env::config::types::starknet::{CallData, FeltPair};
use crate::client::env::Method;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::Repr;
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
        todo!();
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let value: Self::Returns = Decode!(&response, Self::Returns)?;
        Ok(value)
    }
}
