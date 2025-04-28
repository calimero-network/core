#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
use core::{mem, ptr};
use std::collections::BTreeMap;
use std::io::Cursor;

use alloy_sol_types::SolValue;
use candid::{Decode, Encode};
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{BytesN, Env, IntoVal, TryIntoVal};
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode, FeltWriter};
use starknet_crypto::Felt;

use crate::client::env::config::types::ethereum::{SolCapability, SolUserCapabilities};
use crate::client::env::config::types::starknet::{
    CallData, ContextId as StarknetContextId, ContextIdentity as StarknetContextIdentity,
    StarknetPrivileges,
};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::repr::ICRepr;
use crate::icp::types::ICCapability;
use crate::repr::{Repr, ReprTransmute};
use crate::stellar::stellar_types::StellarCapability;
use crate::types::{Capability, ContextId, ContextIdentity, SignerId};

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct PrivilegesRequest<'a> {
    pub(super) context_id: Repr<ContextId>,
    pub(super) identities: &'a [Repr<ContextIdentity>],
}

impl<'a> PrivilegesRequest<'a> {
    pub const fn new(context_id: ContextId, identities: &'a [ContextIdentity]) -> Self {
        // safety: `Repr<T>` is a transparent wrapper around `T`
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
        let context_id = ICRepr::new(*self.context_id);

        // safety:
        //  `Repr<T>` is a transparent wrapper around `T` and
        //  `ICRepr<T>` is a transparent wrapper around `T`

        let identities = unsafe {
            &*(ptr::from_ref::<[Repr<ContextIdentity>]>(self.identities)
                as *const [ICRepr<ContextIdentity>])
        };

        let payload = (context_id, identities);

        Encode!(&payload).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, BTreeMap<ICRepr<SignerId>, Vec<ICCapability>>)?;

        Ok(decoded
            .into_iter()
            .map(|(k, v)| (*k, v.into_iter().map(Into::into).collect()))
            .collect())
    }
}

impl<'a> Method<Stellar> for PrivilegesRequest<'a> {
    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    const METHOD: &'static str = "privileges";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: BytesN<32> = context_id.into_val(&env);

        let mut identities: soroban_sdk::Vec<BytesN<32>> = soroban_sdk::Vec::new(&env);

        for identity in self.identities.iter() {
            let identity_raw: [u8; 32] = identity.rt().expect("infallible conversion");
            identities.push_back(identity_raw.into_val(&env));
        }

        let args = (context_id_val, identities);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let env = Env::default();
        let privileges_map: soroban_sdk::Map<BytesN<32>, soroban_sdk::Vec<StellarCapability>> =
            sc_val
                .try_into_val(&env)
                .map_err(|e| eyre::eyre!("Failed to convert to privileges map: {:?}", e))?;

        // Convert to standard collections
        privileges_map
            .iter()
            .map(|(id, caps)| {
                let signer = id.to_array().rt().expect("infallible conversion");

                let capabilities = caps.iter().map(|cap| cap.into()).collect();

                Ok((signer, capabilities))
            })
            .collect()
    }
}

impl<'a> Method<Ethereum> for PrivilegesRequest<'a> {
    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    const METHOD: &'static str = "privileges";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        let identities: Vec<[u8; 32]> = self
            .identities
            .into_iter()
            .map(|id| id.rt().expect("infallible conversion"))
            .collect();

        Ok((context_id, identities).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let user_caps: Vec<SolUserCapabilities> = SolValue::abi_decode(&response, false)?;

        let mut result = BTreeMap::new();

        for user_cap in user_caps {
            let user_id = user_cap.userId.rt().expect("infallible conversion");

            let capabilities: Result<Vec<_>, _> = user_cap
                .capabilities
                .into_iter()
                .map(|cap| -> Result<_, eyre::Report> {
                    Ok(match cap {
                        SolCapability::ManageApplication => Capability::ManageApplication,
                        SolCapability::ManageMembers => Capability::ManageMembers,
                        SolCapability::Proxy => Capability::Proxy,
                        SolCapability::__Invalid => {
                            eyre::bail!("Invalid capability encountered in response")
                        }
                    })
                })
                .collect();

            if result.insert(user_id, capabilities?).is_some() {
                eyre::bail!("Duplicate user ID in response");
            }
        }

        Ok(result)
    }
}
