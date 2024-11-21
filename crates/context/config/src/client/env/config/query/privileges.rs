use core::{mem, ptr};
use std::collections::BTreeMap;

use serde::Serialize;
use starknet_crypto::Felt;

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
        let (high_bytes, low_bytes) = context_bytes.split_at(context_bytes.len() / 2);

        // Convert to Felts and then to bytes
        let mut result = Vec::new();
        result.extend_from_slice(&Felt::from_bytes_be_slice(high_bytes).to_bytes_be());
        result.extend_from_slice(&Felt::from_bytes_be_slice(low_bytes).to_bytes_be());

        // Add array length
        result.extend_from_slice(&Felt::from(self.identities.len() as u64).to_bytes_be());

        // Add each identity
        for identity in self.identities {
            let id_bytes = identity.as_bytes();
            let (id_high, id_low) = id_bytes.split_at(id_bytes.len() / 2);

            result.extend_from_slice(&Felt::from_bytes_be_slice(id_high).to_bytes_be());
            result.extend_from_slice(&Felt::from_bytes_be_slice(id_low).to_bytes_be());
        }

        Ok(result)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // if response.is_empty() {
        //     return Ok(BTreeMap::new());
        // }

        // let mut result = BTreeMap::new();
        // let mut offset = 0;

        // // First felt is array length
        // let array_len = u64::from_be_bytes(response[24..32].try_into()?);
        // offset += 32;

        // // Process each (identity, capabilities) pair
        // for _ in 0..array_len {
        //     // Read identity from 2 felts (32 bytes each)
        //     let identity = SignerId::from_bytes(|bytes| {
        //         // Take the second felt (low part) for SignerId
        //         bytes.copy_from_slice(&response[offset + 32..offset + 64]);
        //         Ok(32) // Return successful result with number of bytes written
        //     })?;
        //     offset += 64; // Skip both felts (high and low parts)

        //     // Read capability (1 felt)
        //     let cap_value = u64::from_be_bytes(response[offset + 24..offset + 32].try_into()?);
        //     offset += 32;

        //     // Create capabilities vector with single capability
        //     let capabilities = vec![Capability::from(cap_value)];

        //     result.insert(identity, capabilities);
        // }

        // Ok(result)

        todo!()
    }
}
