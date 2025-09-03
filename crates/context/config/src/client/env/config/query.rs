#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
use core::{mem, ptr};
use std::collections::BTreeMap;
use std::io::Cursor;

use alloy::primitives::{Address as AlloyAddress, B256};
use alloy_sol_types::SolValue;
use candid::{Decode, Encode, Principal};
use serde::Serialize;
use soroban_sdk::xdr::{FromXdr, Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{Address, Bytes, BytesN, Env, IntoVal, TryFromVal, TryIntoVal, Val};
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode, FeltWriter};
use starknet_crypto::Felt;

use crate::client::env::config::types::ethereum::{SolApplication, SolCapability, SolUserCapabilities};
use crate::client::env::config::types::starknet::{
    Application as StarknetApplication, CallData, ContextId as StarknetContextId,
    ContextIdentity as StarknetContextIdentity, FeltPair, StarknetMembers, StarknetMembersRequest,
    StarknetPrivileges,
};
use crate::client::env::Method;
use crate::client::env::utils;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::icp::repr::ICRepr;
use crate::icp::types::{ICApplication, ICCapability};
use crate::repr::{Repr, ReprTransmute};
use crate::stellar::stellar_types::{StellarApplication, StellarCapability};
use crate::types::{Application, ApplicationMetadata, ApplicationSource, Capability, ContextId, ContextIdentity, Revision, SignerId};

// Request types for context configuration queries

/// Request to get application information for a context.
#[derive(Copy, Clone, Debug, Serialize)]
pub struct ApplicationRequest {
    pub context_id: Repr<ContextId>,
}

/// Request to get application revision for a context.
#[derive(Copy, Clone, Debug, Serialize)]
pub struct ApplicationRevisionRequest {
    pub context_id: Repr<ContextId>,
}

/// Request to get members of a context with pagination.
#[derive(Copy, Clone, Debug, Serialize)]
pub struct MembersRequest {
    pub context_id: Repr<ContextId>,
    pub offset: usize,
    pub length: usize,
}

/// Request to get members revision for a context.
#[derive(Copy, Clone, Debug, Serialize)]
pub struct MembersRevisionRequest {
    pub context_id: Repr<ContextId>,
}

/// Request to check if a member exists in a context.
#[derive(Copy, Clone, Debug, Serialize)]
pub struct HasMemberRequest {
    pub context_id: Repr<ContextId>,
    pub identity: Repr<ContextIdentity>,
}

/// Request to get privileges for a context.
#[derive(Copy, Clone, Debug, Serialize)]
pub struct PrivilegesRequest<'a> {
    pub context_id: Repr<ContextId>,
    pub identities: &'a [Repr<ContextIdentity>],
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

/// Request to get proxy contract information for a context.
#[derive(Copy, Clone, Debug, Serialize)]
pub struct ProxyContractRequest {
    pub context_id: Repr<ContextId>,
}

/// Request to fetch nonce for a member in a context.
#[derive(Copy, Clone, Debug, Serialize)]
pub struct FetchNonceRequest {
    pub context_id: Repr<ContextId>,
    pub member_id: Repr<ContextIdentity>,
}

impl FetchNonceRequest {
    pub const fn new(context_id: ContextId, member_id: ContextIdentity) -> Self {
        Self {
            context_id: Repr::new(context_id),
            member_id: Repr::new(member_id),
        }
    }
}

#[derive(Debug)]
pub struct ContextConfigQuery<'a, T> {
    pub client: CallClient<'a, T>,
}

impl<'a, T: Transport> ContextConfigQuery<'a, T> {
    pub async fn application(
        &self,
        context_id: ContextId,
    ) -> Result<Application<'static>, ClientError<T>> {
        let params = ApplicationRequest {
            context_id: Repr::new(context_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn application_revision(
        &self,
        context_id: ContextId,
    ) -> Result<Revision, ClientError<T>> {
        let params = ApplicationRevisionRequest {
            context_id: Repr::new(context_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn members(
        &self,
        context_id: ContextId,
        offset: usize,
        length: usize,
    ) -> Result<Vec<ContextIdentity>, ClientError<T>> {
        let params = MembersRequest {
            context_id: Repr::new(context_id),
            offset,
            length,
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn has_member(
        &self,
        context_id: ContextId,
        identity: ContextIdentity,
    ) -> Result<bool, ClientError<T>> {
        let params = HasMemberRequest {
            context_id: Repr::new(context_id),
            identity: Repr::new(identity),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn members_revision(
        &self,
        context_id: ContextId,
    ) -> Result<Revision, ClientError<T>> {
        let params = MembersRevisionRequest {
            context_id: Repr::new(context_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn privileges(
        &self,
        context_id: ContextId,
        identities: &[ContextIdentity],
    ) -> Result<BTreeMap<SignerId, Vec<Capability>>, ClientError<T>> {
        let params = PrivilegesRequest::new(context_id, identities);

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_proxy_contract(
        &self,
        context_id: ContextId,
    ) -> Result<String, ClientError<T>> {
        let params = ProxyContractRequest {
            context_id: Repr::new(context_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn fetch_nonce(
        &self,
        context_id: ContextId,
        member_id: ContextIdentity,
    ) -> Result<Option<u64>, ClientError<T>> {
        let params = FetchNonceRequest::new(context_id, member_id);

        utils::send(&self.client, Operation::Read(params)).await
    }
}

// ApplicationRequest implementations for all protocols

impl Method<Near> for ApplicationRequest {
    const METHOD: &'static str = "application";

    type Returns = Application<'static>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let application: Application<'_> = serde_json::from_slice(&response)?;

        Ok(Application::new(
            application.id,
            application.blob,
            application.size,
            ApplicationSource(application.source.0.into_owned().into()),
            ApplicationMetadata(Repr::new(
                application.metadata.0.into_inner().into_owned().into(),
            )),
        ))
    }
}

impl Method<Starknet> for ApplicationRequest {
    type Returns = Application<'static>;

    const METHOD: &'static str = "application";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let felt_pair: FeltPair = self.context_id.into();
        let mut call_data = CallData::default();
        felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("No application found"));
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

        if felts.is_empty() {
            return Err(eyre::eyre!("No felts decoded from response"));
        }

        // Skip version felt and decode the application
        let application = StarknetApplication::decode(&felts[1..])
            .map_err(|e| eyre::eyre!("Failed to decode application: {:?}", e))?;

        Ok(application.into())
    }
}

impl Method<Icp> for ApplicationRequest {
    type Returns = Application<'static>;

    const METHOD: &'static str = "application";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(self.context_id);
        Encode!(&context_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, ICApplication)?;
        Ok(decoded.into())
    }
}

impl Method<Stellar> for ApplicationRequest {
    type Returns = Application<'static>;

    const METHOD: &'static str = "application";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_val: BytesN<32> = context_raw.into_val(&env);

        let args = (context_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("No application found"));
        }

        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let stellar_application = StellarApplication::from_xdr(&env, &env_bytes)
            .map_err(|_| eyre::eyre!("Failed to deserialize response"))?;

        let application: Application<'_> = stellar_application.into();

        Ok(application)
    }
}

impl Method<Ethereum> for ApplicationRequest {
    type Returns = Application<'static>;

    const METHOD: &'static str = "application(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let application: SolApplication = SolValue::abi_decode(&response, false)?;
        let application: Application<'static> = application.into();

        Ok(application)
    }
}

// ApplicationRevisionRequest implementations for all protocols

impl Method<Near> for ApplicationRevisionRequest {
    const METHOD: &'static str = "application_revision";

    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for ApplicationRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "application_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let felt_pair: FeltPair = self.context_id.into();
        let mut call_data = CallData::default();
        felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 32 bytes, got {}",
                response.len()
            ));
        }

        // Response should be a single u64 in the last 8 bytes of a felt
        let revision_bytes = &response[24..32]; // Take last 8 bytes
        let revision = u64::from_be_bytes(revision_bytes.try_into()?);

        Ok(revision)
    }
}

impl Method<Icp> for ApplicationRevisionRequest {
    type Returns = u64;

    const METHOD: &'static str = "application_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);
        Encode!(&context_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        Decode!(&response, Revision).map_err(Into::into)
    }
}

impl Method<Stellar> for ApplicationRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "application_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: Val = context_id.into_val(&env);

        let args = (context_id_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let revision: u64 = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to u64: {:?}", e))?;
        Ok(revision)
    }
}

impl Method<Ethereum> for ApplicationRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "applicationRevision(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let revision: u64 = SolValue::abi_decode(&response, false)?;

        Ok(revision)
    }
}

// MembersRequest implementations for all protocols

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
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: BytesN<32> = context_id.into_val(&env);

        let offset_val: u32 = self.offset as u32;
        let length_val: u32 = self.length as u32;

        let args = (context_id_val, offset_val, length_val);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let env = Env::default();
        let members: soroban_sdk::Vec<BytesN<32>> = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert to Vec<BytesN<32>>: {:?}", e))?;

        Ok(members
            .iter()
            .map(|id| id.to_array().rt().expect("infallible conversion"))
            .collect())
    }
}

impl Method<Ethereum> for MembersRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "members(bytes32,uint256,uint256)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        let offset_val: u64 = self.offset as u64;
        let length_val: u64 = self.length as u64;

        Ok((context_id, offset_val, length_val).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Decode Vec<B256> directly from response
        let decoded: Vec<B256> = SolValue::abi_decode(&response, false)?;

        // Convert each B256 to ContextIdentity
        Ok(decoded
            .into_iter()
            .map(|b| b.rt().expect("infallible conversion"))
            .collect())
    }
}

// HasMemberRequest implementations for all protocols

impl Method<Near> for HasMemberRequest {
    const METHOD: &'static str = "has_member";

    type Returns = bool;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "has_member";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();

        // Encode context_id
        let context_pair: FeltPair = self.context_id.into();
        context_pair.encode(&mut call_data)?;

        // Encode identity
        let identity_pair: FeltPair = self.identity.into();
        identity_pair.encode(&mut call_data)?;

        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 32 bytes, got {}",
                response.len()
            ));
        }

        // Check if all bytes except the last one are zero
        if !response[..31].iter().all(|&b| b == 0) {
            return Err(eyre::eyre!(
                "Invalid response format: non-zero bytes in prefix"
            ));
        }

        // Check the last byte is either 0 or 1
        match response[31] {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(eyre::eyre!("Invalid boolean value: {}", v)),
        }
    }
}

impl Method<Icp> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "has_member";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut encoded = Vec::new();

        let context_raw: [u8; 32] = self
            .context_id
            .rt()
            .map_err(|e| eyre::eyre!("cannot convert context id to raw bytes: {}", e))?;
        encoded.extend_from_slice(&context_raw);

        let member_raw: [u8; 32] = self
            .identity
            .rt()
            .map_err(|e| eyre::eyre!("cannot convert identity to raw bytes: {}", e))?;
        encoded.extend_from_slice(&member_raw);

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let value = Decode!(&response, Self::Returns)?;
        Ok(value)
    }
}

impl Method<Stellar> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "has_member";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id_bytes: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id: BytesN<32> = context_id_bytes.into_val(&env);
        let identity_bytes: [u8; 32] = self.identity.rt().expect("infallible conversion");
        let identity: BytesN<32> = identity_bytes.into_val(&env);

        let args = (context_id, identity);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let result: bool = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to bool: {:?}", e))?;

        Ok(result)
    }
}

impl Method<Ethereum> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "hasMember(bytes32,bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let identity_bytes: [u8; 32] = self.identity.rt().expect("infallible conversion");

        Ok((context_id, identity_bytes).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let result: bool = SolValue::abi_decode(&response, false)?;
        Ok(result)
    }
}

// MembersRevisionRequest implementations for all protocols

impl Method<Near> for MembersRevisionRequest {
    const METHOD: &'static str = "members_revision";

    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for MembersRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "members_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Dereference Repr and encode context_id
        let context_id: StarknetContextId = (*self.context_id).into();

        let mut call_data = CallData::default();
        context_id.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 32 bytes, got {}",
                response.len()
            ));
        }

        // Response should be a single u64 in the last 8 bytes of a felt
        // First 24 bytes should be zero
        if !response[..24].iter().all(|&b| b == 0) {
            return Err(eyre::eyre!(
                "Invalid response format: non-zero bytes in prefix"
            ));
        }

        let revision_bytes = &response[24..32];
        let revision = u64::from_be_bytes(revision_bytes.try_into()?);

        Ok(revision)
    }
}

impl Method<Icp> for MembersRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "members_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);
        Encode!(&context_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let value = Decode!(&response, Self::Returns)?;
        Ok(value)
    }
}

impl Method<Stellar> for MembersRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "members_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: BytesN<32> = context_id.into_val(&env);

        let args = (context_id_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let revision: u64 = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to u64: {:?}", e))?;
        Ok(revision)
    }
}

impl Method<Ethereum> for MembersRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "membersRevision(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let revision: u64 = SolValue::abi_decode(&response, false)?;

        Ok(revision)
    }
}

// PrivilegesRequest implementations for all protocols

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

// ProxyContractRequest implementations for all protocols

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

// FetchNonceRequest implementations for all protocols

impl Method<Near> for FetchNonceRequest {
    const METHOD: &'static str = "fetch_nonce";

    type Returns = Option<u64>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();

        // Dereference Repr and encode context_id
        let context_id: StarknetContextId = (*self.context_id).into();
        context_id.encode(&mut call_data)?;

        let member_id: StarknetContextIdentity = (*self.member_id).into();
        member_id.encode(&mut call_data)?;

        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 8 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 8 bytes, got {}",
                response.len()
            ));
        }

        let nonce = u64::from_be_bytes(
            response
                .try_into()
                .map_err(|_| eyre::eyre!("Failed to convert response to u64"))?,
        );

        Ok(Some(nonce))
    }
}

impl Method<Icp> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);
        let member_id = ICRepr::new(*self.member_id);

        // Encode arguments separately
        Encode!(&context_id, &member_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, Option<u64>)?;

        Ok(decoded)
    }
}

impl Method<Stellar> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: BytesN<32> = context_id.into_val(&env);

        let member_id: [u8; 32] = self.member_id.rt().expect("infallible conversion");
        let member_id_val: BytesN<32> = member_id.into_val(&env);

        let args = (context_id_val, member_id_val);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let nonce: u64 = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to u64: {:?}", e))?;

        Ok(Some(nonce))
    }
}

impl Method<Ethereum> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetchNonce(bytes32,bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let member_id: [u8; 32] = self.member_id.rt().expect("infallible conversion");

        Ok((context_id, member_id).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let nonce: u64 = SolValue::abi_decode(&response, false)?;

        Ok(Some(nonce))
    }
}
