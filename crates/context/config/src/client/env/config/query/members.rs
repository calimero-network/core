use core::mem;

use candid::{Decode, Encode};
use serde::Serialize;
use soroban_sdk::xdr::FromXdr;
use soroban_sdk::{Bytes, BytesN, Env};
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode};
use starknet_crypto::Felt;

use crate::client::env::config::types::starknet::{
    CallData, StarknetMembers, StarknetMembersRequest,
};
use crate::client::env::Method;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::repr::ICRepr;
use crate::repr::{Repr, ReprBytes, ReprTransmute};
use crate::stellar::stellar_repr::StellarRepr;
use crate::types::{ContextId, ContextIdentity};

#[derive(Copy, Clone, Debug, Serialize)]
pub(crate) struct MembersRequest {
    pub(crate) context_id: Repr<ContextId>,
    pub(crate) offset: usize,
    pub(crate) length: usize,
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
        let mut call_data = CallData::default();
        req.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(Vec::new());
        }

        if response.len() % 32 != 0 {
            return Err(eyre::eyre!(
                "Invalid response length: {} bytes is not a multiple of 32",
                response.len()
            ));
        }

        // Convert bytes to Felts
        let mut felts = Vec::new();
        let chunks = response.chunks_exact(32);

        // Verify no remainder
        if !chunks.remainder().is_empty() {
            return Err(eyre::eyre!("Response length is not a multiple of 32 bytes"));
        }

        for chunk in chunks {
            let chunk_array: [u8; 32] = chunk
                .try_into()
                .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
            felts.push(Felt::from_bytes_be(&chunk_array));
        }

        // Check if it's a None response (single zero Felt)
        if felts.len() == 1 && felts[0] == Felt::ZERO {
            return Ok(Vec::new());
        }

        // Decode directly from the felts slice - the Decode trait will handle the array structure
        let members = StarknetMembers::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode members: {:?}", e))?;

        Ok(members.into())
    }
}

impl Method<Icp> for MembersRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "members";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);

        Encode!(&context_id, &self.offset, &self.length).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let members = Decode!(&response, Vec<ICRepr<ContextIdentity>>)?;

        // safety: `ICRepr<T>` is a transparent wrapper around `T`
        #[expect(
            clippy::transmute_undefined_repr,
            reason = "ICRepr<T> is a transparent wrapper around T"
        )]
        let members = unsafe {
            mem::transmute::<Vec<ICRepr<ContextIdentity>>, Vec<ContextIdentity>>(members)
        };

        Ok(members)
    }
}

impl Method<Stellar> for MembersRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "members";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut encoded = Vec::new();

        // Encode context_id (BytesN<32>)
        let context_raw: [u8; 32] = self.context_id.rt().expect("context does not exist");
        encoded.extend_from_slice(&context_raw);

        // Encode offset (u32)
        encoded.extend_from_slice(&self.offset.to_le_bytes());

        // Encode length (u32)
        encoded.extend_from_slice(&self.length.to_le_bytes());

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("No members found"));
        }

        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let members = soroban_sdk::Vec::<BytesN<32>>::from_xdr(&env, &env_bytes)
            .map_err(|_| eyre::eyre!("Failed to deserialize members"))?;

        Ok(members
            .iter()
            .map(|id| {
                ContextIdentity::from_bytes(|dest| {
                    dest.copy_from_slice(&id.to_array());
                    Ok(32)
                })
                .expect("Valid 32-byte array")
            })
            .collect())
    }
}
