use serde::Serialize;
use starknet_crypto::Felt;

use crate::client::env::proxy::types::starknet::StarknetProposal;
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::Proposal;
use starknet::core::codec::Decode;


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
        let mut bytes = Vec::with_capacity(64); // 2 * 32 bytes for two u32 parameters
      
        // First parameter (offset): pad to 32 bytes
        bytes.extend_from_slice(&[0; 28]); // 28 bytes of zeros
        bytes.extend_from_slice(&(self.offset as u32).to_be_bytes()); // 4 bytes of actual value
        
        // Second parameter (length): pad to 32 bytes
        bytes.extend_from_slice(&[0; 28]); // 28 bytes of zeros
        bytes.extend_from_slice(&(self.length as u32).to_be_bytes()); // 4 bytes of actual value

        Ok(bytes)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // First, convert bytes back to Felts
        let mut felts = Vec::new();
        for chunk in response.chunks(32) {
            if chunk.len() == 32 {
                felts.push(Felt::from_bytes_be(chunk.try_into().unwrap()));
            }
        }
        // println!("Felts: {:?}", felts);

        // First felt is array length
        let array_len = felts[0].to_bytes_be()[31] as usize;
        // println!("Array length: {}", array_len);

        let mut proposals = Vec::with_capacity(array_len);
        let mut offset = 1; // Skip the length felt

        for i in 0..array_len {
            // Each proposal starts with:
            // - proposal_id: 2 felts (high, low)
            // - author_id: 2 felts (high, low)
            // - action variant: 1 felt
            let variant = felts[offset + 4].to_bytes_be()[31];
            // println!("Proposal {} at offset {}, variant {}", i, offset, variant);
    
            let proposal_end = match variant {
                0 => { // ExternalFunctionCall
                    let calldata_len = felts[offset + 7].to_bytes_be()[31] as usize;
                    offset + 8 + calldata_len // base + contract + selector + len + calldata
                },
                1 => { // Transfer
                    offset + 9 // base + token + amount(2) + receiver
                },
                2 => { // SetNumApprovals
                    offset + 6 // base + num
                },
                3 => { // SetActiveProposalsLimit
                    offset + 6 // base + limit
                },
                4 => { // SetContextValue
                    let key_len = felts[offset + 5].to_bytes_be()[31] as usize;
                    let value_len = felts[offset + 6 + key_len].to_bytes_be()[31] as usize;
                    offset + 7 + key_len + value_len // base + key_len + value_len + key + value
                },
                _ => return Err(eyre::eyre!("Unknown action variant: {}", variant)),
            };
    
            let proposal = StarknetProposal::decode(&felts[offset..proposal_end])
                .map_err(|e| eyre::eyre!("Failed to decode proposal {}: {:?}", i, e))?;
            // println!("Decoded proposal {}: {:?}", i, proposal);
            
            proposals.push(proposal);
            offset = proposal_end;
        }
        // println!("Proposals: {:?}", proposals);

        Ok(proposals.into_iter().map(|p| p.into()).collect())
    }
}
