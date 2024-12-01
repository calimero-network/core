use core::{mem, ptr};
use std::collections::BTreeMap;

use candid::{Decode, Encode};
use serde::Serialize;
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode, FeltWriter};
use starknet_crypto::Felt;

use crate::client::env::config::types::icp::{
    ICCapability, ICContextId, ICContextIdentity, ICSignerId,
};
use crate::client::env::config::types::starknet::{
    CallData, ContextId as StarknetContextId, ContextIdentity as StarknetContextIdentity,
    StarknetPrivileges,
};
use crate::client::env::Method;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::{Repr, ReprTransmute};
use crate::types::{Capability, ContextId, ContextIdentity, SignerId};

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct PrivilegesRequest<'a> {
    pub(super) context_id: Repr<ContextId>,
    pub(super) identities: &'a [Repr<ContextIdentity>],
}

impl<'a> PrivilegesRequest<'a> {
    pub const fn new(context_id: ContextId, identities: &'a [ContextIdentity]) -> Self {
        let identities = unsafe {
            &*(ptr::from_ref::<[ContextIdentity]>(identities) as *const [Repr<ContextIdentity>])
        };

        Self {
            context_id: Repr::new(context_id),
            identities,
        }
    }
}

impl<'a> Method<Near> for PrivilegesRequest<'a> {
    const METHOD: &'static str = "privileges";

    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let privileges: BTreeMap<Repr<SignerId>, Vec<Capability>> =
            serde_json::from_slice(&response)?;

        // safety: `Repr<T>` is a transparent wrapper around `T`
        let privileges = unsafe {
            #[expect(
                clippy::transmute_undefined_repr,
                reason = "Repr<T> is a transparent wrapper around T"
            )]
            mem::transmute::<
                BTreeMap<Repr<SignerId>, Vec<Capability>>,
                BTreeMap<SignerId, Vec<Capability>>,
            >(privileges)
        };

        Ok(privileges)
    }
}

impl<'a> Method<Starknet> for PrivilegesRequest<'a> {
    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    const METHOD: &'static str = "privileges";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();

        // Dereference Repr and encode context_id
        let context_id: StarknetContextId = (*self.context_id).into();
        context_id.encode(&mut call_data)?;

        // Add array length
        call_data.write(Felt::from(self.identities.len() as u64));

        // Add each identity using StarknetIdentity
        for identity in self.identities {
            let starknet_id: StarknetContextIdentity = (*identity).into();
            starknet_id.encode(&mut call_data)?;
        }

        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(BTreeMap::new());
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
            return Ok(BTreeMap::new());
        }

        let privileges = StarknetPrivileges::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode privileges: {:?}", e))?;

        Ok(privileges.into())
    }
}

impl<'a> Method<Icp> for PrivilegesRequest<'a> {
    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    const METHOD: &'static str = "privileges";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Convert context_id and identities to ICP types
        let context_id: ICContextId = (*self.context_id).rt()?;
        let identities: Vec<ICContextIdentity> = self
            .identities
            .iter()
            .map(|id| (*id).rt())
            .collect::<Result<Vec<_>, _>>()?;

        // Create a tuple of the values we want to encode
        let payload = (context_id, identities);

        // Encode using Candid
        Encode!(&payload).map_err(|e| eyre::eyre!("Failed to encode privileges request: {}", e))
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded: BTreeMap<ICSignerId, Vec<ICCapability>> =
            Decode!(&response, BTreeMap<ICSignerId, Vec<ICCapability>>)?;
        Ok(decoded
            .into_iter()
            .map(|(k, v)| (k.into(), v.into_iter().map(Into::into).collect()))
            .collect())
    }
}
