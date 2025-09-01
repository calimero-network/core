//! NEAR-specific query implementations.

use std::collections::BTreeMap;

use serde::Serialize;

use calimero_context_config_core::repr::{Repr, ReprBytes};
use calimero_context_config_core::types::{Application, Capability, ContextId, ContextIdentity, Revision, SignerId};

// Trait for method implementations
pub trait Method<Protocol> {
    type Returns;
    const METHOD: &'static str;

    fn encode(self) -> eyre::Result<Vec<u8>>;
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns>;
}

// NEAR protocol marker
pub struct Near;

#[derive(Copy, Clone, Debug, Serialize)]
pub struct ApplicationRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Near> for ApplicationRequest {
    const METHOD: &'static str = "application";
    type Returns = Application<'static>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Near>>::Returns> {
        // For now, return a default application as the full implementation would require
        // proper deserialization of core types
        let context_id: [u8; 32] = [0u8; 32]; // Default context ID
        let application_id = Repr::new(calimero_context_config_core::types::ApplicationId::from_bytes(|bytes| {
            bytes.copy_from_slice(&context_id);
            Ok(32)
        }).expect("Failed to create ApplicationId"));
        let blob_id = Repr::new(calimero_context_config_core::types::BlobId::from_bytes(|bytes| {
            bytes.copy_from_slice(&context_id);
            Ok(32)
        }).expect("Failed to create BlobId"));

        Ok(Application::new(
            application_id,
            blob_id,
            0,
            calimero_context_config_core::types::ApplicationSource(std::borrow::Cow::Owned("".to_string())),
            calimero_context_config_core::types::ApplicationMetadata(Repr::new(std::borrow::Cow::Owned(vec![]))),
        ))
    }
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct MembersRequest {
    pub context_id: Repr<ContextId>,
    pub offset: u32,
    pub length: u32,
}

impl Method<Near> for MembersRequest {
    const METHOD: &'static str = "members";
    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Near>>::Returns> {
        // For now, return empty members as the full implementation would require
        // proper deserialization of core types
        Ok(Vec::new())
    }
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct ApplicationRevisionRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Near> for ApplicationRevisionRequest {
    const METHOD: &'static str = "application_revision";
    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Near>>::Returns> {
        let revision: Revision = serde_json::from_slice(&response)?;
        Ok(revision)
    }
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct HasMemberRequest {
    pub context_id: Repr<ContextId>,
    pub identity: Repr<ContextIdentity>,
}

impl Method<Near> for HasMemberRequest {
    const METHOD: &'static str = "has_member";
    type Returns = bool;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Near>>::Returns> {
        let result: bool = serde_json::from_slice(&response)?;
        Ok(result)
    }
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct MembersRevisionRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Near> for MembersRevisionRequest {
    const METHOD: &'static str = "members_revision";
    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Near>>::Returns> {
        let revision: Revision = serde_json::from_slice(&response)?;
        Ok(revision)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PrivilegesRequest {
    pub context_id: Repr<ContextId>,
    pub identities: Vec<Repr<ContextIdentity>>,
}

impl Method<Near> for PrivilegesRequest {
    const METHOD: &'static str = "privileges";
    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Near>>::Returns> {
        // For now, return empty privileges as the full implementation would require
        // proper deserialization of core types
        Ok(BTreeMap::new())
    }
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct ProxyContractRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Near> for ProxyContractRequest {
    const METHOD: &'static str = "proxy_contract";
    type Returns = String;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Near>>::Returns> {
        let contract_address: String = serde_json::from_slice(&response)?;
        Ok(contract_address)
    }
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct FetchNonceRequest {
    pub context_id: Repr<ContextId>,
    pub member_id: Repr<ContextIdentity>,
}

impl Method<Near> for FetchNonceRequest {
    const METHOD: &'static str = "fetch_nonce";
    type Returns = Option<u64>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Near>>::Returns> {
        let nonce: Option<u64> = serde_json::from_slice(&response)?;
        Ok(nonce)
    }
}
