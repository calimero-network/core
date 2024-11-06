use core::mem;

use serde::Serialize;
use starknet::core::codec::Encode;
use starknet_crypto::Felt;

use crate::client::env::config::types::starknet::StarknetMembersRequest;
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::{Repr, ReprBytes, ReprTransmute};
use crate::types::{ContextId, ContextIdentity};

#[derive(Copy, Clone, Debug, Serialize)]
pub struct MembersRequest {
    pub context_id: Repr<ContextId>,
    pub offset: usize,
    pub length: usize,
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
        let mut serialized_request = vec![];
        req.encode(&mut serialized_request).unwrap();

        let bytes: Vec<u8> = serialized_request
            .iter()
            .flat_map(|felt| felt.to_bytes_be())
            .collect();

        Ok(bytes)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(Vec::new());
        }

        // First 32 bytes contain the count, skip it
        let response = &response[32..];

        let members: Result<Vec<ContextIdentity>, _> = response
            .chunks_exact(64)
            .map(|chunk| {
                let felt1 = Felt::from_bytes_be_slice(&chunk[..32]);
                let felt2 = Felt::from_bytes_be_slice(&chunk[32..]);

                let felt1_bytes = felt1.to_bytes_be();
                let felt2_bytes = felt2.to_bytes_be();

                ContextIdentity::from_bytes(|bytes| {
                    bytes[..16].copy_from_slice(&felt1_bytes[16..]);
                    bytes[16..].copy_from_slice(&felt2_bytes[16..]);
                    Ok(32)
                })
            })
            .collect();

        let members = members.map_err(|e| eyre::eyre!("Failed to decode members: {:?}", e))?;
        Ok(members)
    }
}
