//! ICP-specific types and implementations for Calimero context configuration.

use std::borrow::Cow;

use candid::CandidType;
use serde::Deserialize;

use calimero_context_config_core::repr::ReprBytes;

// ICP-specific application type
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ICApplication {
    pub id: Vec<u8>,
    pub blob: Vec<u8>,
    pub size: u64,
    pub source: String,
    pub metadata: Vec<u8>,
}

// ICP-specific capability type
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ICCapability {
    ManageApplication,
    ManageMembers,
    Proxy,
}

// ICP-specific request types
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ICContextRequest {
    pub context_id: Vec<u8>,
    pub kind: ICContextRequestKind,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ICContextRequestKind {
    Add(Vec<u8>, ICApplication),
    UpdateApplication(ICApplication),
    AddMembers(Vec<Vec<u8>>),
    RemoveMembers(Vec<Vec<u8>>),
    Grant(Vec<(Vec<u8>, ICCapability)>),
    Revoke(Vec<(Vec<u8>, ICCapability)>),
    UpdateProxyContract,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ICRequestKind {
    Context(ICContextRequest),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ICRequest {
    pub kind: ICRequestKind,
    pub signer_id: Vec<u8>,
    pub nonce: u64,
}

// Add conversion from ICP Capability to domain Capability
impl From<ICCapability> for calimero_context_config_core::types::Capability {
    fn from(value: ICCapability) -> Self {
        match value {
            ICCapability::ManageApplication => calimero_context_config_core::types::Capability::ManageApplication,
            ICCapability::ManageMembers => calimero_context_config_core::types::Capability::ManageMembers,
            ICCapability::Proxy => calimero_context_config_core::types::Capability::Proxy,
        }
    }
}

// Add conversion from ICP Application to domain Application
impl<'a> From<ICApplication> for calimero_context_config_core::types::Application<'a> {
    fn from(value: ICApplication) -> Self {
        use calimero_context_config_core::repr::Repr;
        use calimero_context_config_core::types::{ApplicationId, BlobId};
        
        let application_id = Repr::new(ApplicationId::from_bytes(|bytes| {
            bytes.copy_from_slice(&value.id);
            Ok(32)
        }).expect("Failed to create ApplicationId"));
        
        let blob_id = Repr::new(BlobId::from_bytes(|bytes| {
            bytes.copy_from_slice(&value.blob);
            Ok(32)
        }).expect("Failed to create BlobId"));

        calimero_context_config_core::types::Application::new(
            application_id,
            blob_id,
            value.size,
            calimero_context_config_core::types::ApplicationSource(std::borrow::Cow::Owned(value.source)),
            calimero_context_config_core::types::ApplicationMetadata(Repr::new(std::borrow::Cow::Owned(value.metadata))),
        )
    }
}
