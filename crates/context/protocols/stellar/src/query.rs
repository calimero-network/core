//! Stellar-specific query implementations.

use std::collections::BTreeMap;

use soroban_sdk::xdr::{FromXdr, Limits, ReadXdr, ToXdr};
use soroban_sdk::{Address, Bytes, BytesN, Env, IntoVal};

use calimero_context_config_core::repr::{Repr, ReprTransmute};
use calimero_context_config_core::types::{Application, Capability, ContextId, ContextIdentity, Revision, SignerId};

use crate::types::{StellarApplication, StellarCapability};

// Trait for method implementations
pub trait Method<Protocol> {
    type Returns;
    const METHOD: &'static str;

    fn encode(self) -> eyre::Result<Vec<u8>>;
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns>;
}

// Stellar protocol marker
pub struct Stellar;

#[derive(Copy, Clone, Debug)]
pub struct ApplicationRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Stellar> for ApplicationRequest {
    const METHOD: &'static str = "application";
    type Returns = Application<'static>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_val: BytesN<32> = context_raw.into_val(&env);

        let args = (context_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Stellar>>::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("No application found"));
        }

        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let stellar_application = StellarApplication::from_xdr(&env, &env_bytes)
            .map_err(|_| eyre::eyre!("Failed to deserialize response"))?;

        Ok(stellar_application.into())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct MembersRequest {
    pub context_id: Repr<ContextId>,
    pub offset: u32,
    pub length: u32,
}

impl Method<Stellar> for MembersRequest {
    const METHOD: &'static str = "members";
    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_val: BytesN<32> = context_raw.into_val(&env);

        let args = (context_val, self.offset, self.length);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Stellar>>::Returns> {
        if response.is_empty() {
            return Ok(Vec::new());
        }

        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        // For now, return empty vector as Vec<BytesN<32>> doesn't implement FromXdr
        // This would need to be implemented based on the actual Stellar contract response format
        let members: Vec<BytesN<32>> = Vec::new();

        Ok(members
            .into_iter()
            .map(|member| member.to_array().rt().expect("infallible conversion"))
            .collect())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ApplicationRevisionRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Stellar> for ApplicationRevisionRequest {
    const METHOD: &'static str = "application_revision";
    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_val: BytesN<32> = context_raw.into_val(&env);

        let args = (context_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Stellar>>::Returns> {
        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let revision: u64 = <u64 as ReadXdr>::from_xdr(&response, Limits::none())
            .map_err(|_| eyre::eyre!("Failed to deserialize response"))?;

        Ok(revision)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct HasMemberRequest {
    pub context_id: Repr<ContextId>,
    pub identity: Repr<ContextIdentity>,
}

impl Method<Stellar> for HasMemberRequest {
    const METHOD: &'static str = "has_member";
    type Returns = bool;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let identity_raw: [u8; 32] = self.identity.rt().expect("infallible conversion");

        let context_val: BytesN<32> = context_raw.into_val(&env);
        let identity_val: BytesN<32> = identity_raw.into_val(&env);

        let args = (context_val, identity_val);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Stellar>>::Returns> {
        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let result: bool = <bool as ReadXdr>::from_xdr(&response, Limits::none())
            .map_err(|_| eyre::eyre!("Failed to deserialize response"))?;

        Ok(result)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct MembersRevisionRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Stellar> for MembersRevisionRequest {
    const METHOD: &'static str = "members_revision";
    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_val: BytesN<32> = context_raw.into_val(&env);

        let args = (context_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Stellar>>::Returns> {
        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let revision: u64 = <u64 as ReadXdr>::from_xdr(&response, Limits::none())
            .map_err(|_| eyre::eyre!("Failed to deserialize response"))?;

        Ok(revision)
    }
}

#[derive(Clone, Debug)]
pub struct PrivilegesRequest {
    pub context_id: Repr<ContextId>,
    pub identities: Vec<Repr<ContextIdentity>>,
}

impl Method<Stellar> for PrivilegesRequest {
    const METHOD: &'static str = "privileges";
    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_val: BytesN<32> = context_raw.into_val(&env);

        // For now, encode just the context_id as the full implementation would require
        // proper XDR serialization of Vec<BytesN<32>>
        let args = (context_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Stellar>>::Returns> {
        if response.is_empty() {
            return Ok(BTreeMap::new());
        }

        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        // For now, return empty privileges as the full implementation would require
        // proper XDR deserialization of Vec<(BytesN<32>, Vec<StellarCapability>)>
        let privileges: Vec<(BytesN<32>, Vec<StellarCapability>)> = Vec::new();

        let mut result = BTreeMap::new();

        for (identity, capabilities) in privileges {
            let identity_bytes: [u8; 32] = identity.to_array();
            let signer_id: SignerId = identity_bytes.rt().expect("infallible conversion");

            let converted_capabilities: Vec<Capability> = capabilities
                .into_iter()
                .map(|cap| cap.into())
                .collect();

            if result.insert(signer_id, converted_capabilities).is_some() {
                eyre::bail!("Duplicate signer ID in response");
            }
        }

        Ok(result)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ProxyContractRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Stellar> for ProxyContractRequest {
    const METHOD: &'static str = "proxy_contract";
    type Returns = String;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_val: BytesN<32> = context_raw.into_val(&env);

        let args = (context_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Stellar>>::Returns> {
        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let contract_address: Address = Address::from_xdr(&env, &env_bytes)
            .map_err(|_| eyre::eyre!("Failed to deserialize response"))?;

        Ok(contract_address.to_string().to_string())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct FetchNonceRequest {
    pub context_id: Repr<ContextId>,
    pub member_id: Repr<ContextIdentity>,
}

impl Method<Stellar> for FetchNonceRequest {
    const METHOD: &'static str = "fetch_nonce";
    type Returns = Option<u64>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let member_raw: [u8; 32] = self.member_id.rt().expect("infallible conversion");

        let context_val: BytesN<32> = context_raw.into_val(&env);
        let member_val: BytesN<32> = member_raw.into_val(&env);

        let args = (context_val, member_val);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Stellar>>::Returns> {
        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let nonce: u64 = <u64 as ReadXdr>::from_xdr(&response, Limits::none())
            .map_err(|_| eyre::eyre!("Failed to deserialize response"))?;

        if nonce == 0 {
            Ok(None)
        } else {
            Ok(Some(nonce))
        }
    }
}
