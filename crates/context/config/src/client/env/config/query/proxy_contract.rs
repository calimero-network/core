use serde::Serialize;
use starknet_crypto::Felt;

use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::Repr;
use crate::repr::ReprBytes;
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
        // Split context_id into high/low parts
        let bytes = self.context_id.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);

        // Convert to Felts
        let high_felt = Felt::from_bytes_be_slice(high_bytes);
        let low_felt = Felt::from_bytes_be_slice(low_bytes);

        // Convert both Felts to bytes and concatenate
        let mut result = Vec::new();
        result.extend_from_slice(&high_felt.to_bytes_be());
        result.extend_from_slice(&low_felt.to_bytes_be());
        Ok(result)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        println!("response {:?}", response);
        if response.is_empty() {
            return Err(eyre::eyre!("No proxy contract found"));
        }

        // Check if it's a None response (single zero Felt)
        if response.iter().all(|&x| x == 0) {
            return Err(eyre::eyre!("No proxy contract found"));
        }

        // Convert the Felt to a hex string representing the contract address
        let hex_string = format!("0x{}", hex::encode(&response));
        
        Ok(hex_string)
    }
}
