use core::{mem, ptr};
use std::collections::BTreeMap;

use serde::Serialize;
use starknet::core::codec::Decode;
use starknet_crypto::Felt;

use crate::client::env::config::types::starknet::StarknetPrivileges;
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::{Repr, ReprBytes};
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
        // Split context_id into high/low parts
        let context_bytes = self.context_id.as_bytes();
        let mid_point = context_bytes.len().checked_div(2).expect("Length should be even");
        let (high_bytes, low_bytes) = context_bytes.split_at(mid_point);

        // Convert to Felts and then to bytes
        let mut result = Vec::new();
        result.extend_from_slice(&Felt::from_bytes_be_slice(high_bytes).to_bytes_be());
        result.extend_from_slice(&Felt::from_bytes_be_slice(low_bytes).to_bytes_be());

        // Add array length
        result.extend_from_slice(&Felt::from(self.identities.len() as u64).to_bytes_be());

        // Add each identity
        for identity in self.identities {
            let id_bytes = identity.as_bytes();
            let mid_point = id_bytes.len().checked_div(2).expect("Length should be even");
            let (id_high, id_low) = id_bytes.split_at(mid_point);

            result.extend_from_slice(&Felt::from_bytes_be_slice(id_high).to_bytes_be());
            result.extend_from_slice(&Felt::from_bytes_be_slice(id_low).to_bytes_be());
        }

        Ok(result)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(BTreeMap::new());
        }

        // Convert bytes to Felts
        let mut felts = Vec::new();
        for chunk in response.chunks(32) {
            if chunk.len() == 32 {
                felts.push(Felt::from_bytes_be(chunk.try_into().map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?));
            }
        }

        // Check if it's a None response (single zero Felt)
        if felts.len() == 1 && felts[0] == Felt::ZERO {
            return Ok(BTreeMap::new());
        }

        // Skip the flag/version felt and decode the privileges
        let privileges = StarknetPrivileges::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode privileges: {:?}", e))?;

        Ok(privileges.into())
    }
}
