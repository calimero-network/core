//! ICP-specific query implementations.

use std::collections::BTreeMap;

use candid::{Decode, Encode};

use calimero_context_config_core::repr::{Repr, ReprTransmute};
use calimero_context_config_core::types::{Application, Capability, ContextId, ContextIdentity, Revision, SignerId};

use crate::types::{ICApplication, ICCapability};

// Trait for method implementations
pub trait Method<Protocol> {
    type Returns;
    const METHOD: &'static str;

    fn encode(self) -> eyre::Result<Vec<u8>>;
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns>;
}

// ICP protocol marker
pub struct Icp;

#[derive(Copy, Clone, Debug)]
pub struct ApplicationRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Icp> for ApplicationRequest {
    const METHOD: &'static str = "application";
    type Returns = Application<'static>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id_bytes: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        Encode!(&context_id_bytes.to_vec()).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Icp>>::Returns> {
        let decoded = Decode!(&response, ICApplication)?;
        Ok(decoded.into())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct MembersRequest {
    pub context_id: Repr<ContextId>,
    pub offset: usize,
    pub length: usize,
}

impl Method<Icp> for MembersRequest {
    const METHOD: &'static str = "members";
    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id_bytes: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        Encode!(&context_id_bytes.to_vec(), &self.offset, &self.length).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Icp>>::Returns> {
        let members: Vec<Vec<u8>> = Decode!(&response, Vec<Vec<u8>>)?;
        Ok(members
            .into_iter()
            .map(|member_bytes| {
                let mut bytes = [0u8; 32];
                bytes[..member_bytes.len().min(32)].copy_from_slice(&member_bytes[..member_bytes.len().min(32)]);
                bytes.rt().expect("infallible conversion")
            })
            .collect())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ApplicationRevisionRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Icp> for ApplicationRevisionRequest {
    const METHOD: &'static str = "application_revision";
    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id_bytes: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        Encode!(&context_id_bytes.to_vec()).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Icp>>::Returns> {
        let revision: Revision = Decode!(&response, Revision)?;
        Ok(revision)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct HasMemberRequest {
    pub context_id: Repr<ContextId>,
    pub identity: Repr<ContextIdentity>,
}

impl Method<Icp> for HasMemberRequest {
    const METHOD: &'static str = "has_member";
    type Returns = bool;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id_bytes: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let identity_bytes: [u8; 32] = self.identity.rt().expect("infallible conversion");
        Encode!(&context_id_bytes.to_vec(), &identity_bytes.to_vec()).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Icp>>::Returns> {
        let result: bool = Decode!(&response, bool)?;
        Ok(result)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct MembersRevisionRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Icp> for MembersRevisionRequest {
    const METHOD: &'static str = "members_revision";
    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id_bytes: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        Encode!(&context_id_bytes.to_vec()).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Icp>>::Returns> {
        let revision: Revision = Decode!(&response, Revision)?;
        Ok(revision)
    }
}

#[derive(Clone, Debug)]
pub struct PrivilegesRequest {
    pub context_id: Repr<ContextId>,
    pub identities: Vec<Repr<ContextIdentity>>,
}

impl Method<Icp> for PrivilegesRequest {
    const METHOD: &'static str = "privileges";
    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id_bytes: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let identities: Vec<Vec<u8>> = self
            .identities
            .into_iter()
            .map(|id| {
                let id_bytes: [u8; 32] = id.rt().expect("infallible conversion");
                id_bytes.to_vec()
            })
            .collect();
        Encode!(&context_id_bytes.to_vec(), &identities).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Icp>>::Returns> {
        let privileges: Vec<(Vec<u8>, Vec<ICCapability>)> = Decode!(&response, Vec<(Vec<u8>, Vec<ICCapability>)>)?;
        
        let mut result = BTreeMap::new();
        for (identity_bytes, capabilities) in privileges {
            let mut bytes = [0u8; 32];
            bytes[..identity_bytes.len().min(32)].copy_from_slice(&identity_bytes[..identity_bytes.len().min(32)]);
            let signer_id: SignerId = bytes.rt().expect("infallible conversion");
            
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

impl Method<Icp> for ProxyContractRequest {
    const METHOD: &'static str = "proxy_contract";
    type Returns = String;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id_bytes: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        Encode!(&context_id_bytes.to_vec()).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Icp>>::Returns> {
        let contract_address: String = Decode!(&response, String)?;
        Ok(contract_address)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct FetchNonceRequest {
    pub context_id: Repr<ContextId>,
    pub member_id: Repr<ContextIdentity>,
}

impl Method<Icp> for FetchNonceRequest {
    const METHOD: &'static str = "fetch_nonce";
    type Returns = Option<u64>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id_bytes: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let member_id_bytes: [u8; 32] = self.member_id.rt().expect("infallible conversion");
        Encode!(&context_id_bytes.to_vec(), &member_id_bytes.to_vec()).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Icp>>::Returns> {
        let nonce: Option<u64> = Decode!(&response, Option<u64>)?;
        Ok(nonce)
    }
}
