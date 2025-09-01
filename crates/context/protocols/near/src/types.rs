//! NEAR-specific types and implementations for Calimero context configuration.

use std::collections::BTreeMap;

use serde::Serialize;

use calimero_context_config_core::repr::Repr;
use calimero_context_config_core::types::{
    Application, Capability, ContextId,
    ContextIdentity, Revision, SignerId,
};

// NEAR-specific request types
#[derive(Copy, Clone, Debug, Serialize)]
pub struct ApplicationRequest {
    pub context_id: Repr<ContextId>,
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct MembersRequest {
    pub context_id: Repr<ContextId>,
    pub offset: u32,
    pub length: u32,
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct ApplicationRevisionRequest {
    pub context_id: Repr<ContextId>,
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct HasMemberRequest {
    pub context_id: Repr<ContextId>,
    pub identity: Repr<ContextIdentity>,
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct MembersRevisionRequest {
    pub context_id: Repr<ContextId>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PrivilegesRequest {
    pub context_id: Repr<ContextId>,
    pub identities: Vec<Repr<ContextIdentity>>,
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct ProxyContractRequest {
    pub context_id: Repr<ContextId>,
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct NonceRequest {
    pub context_id: Repr<ContextId>,
    pub member_id: Repr<ContextIdentity>,
}

// NEAR-specific response types
pub type ApplicationResponse = Application<'static>;
pub type MembersResponse = Vec<ContextIdentity>;
pub type ApplicationRevisionResponse = Revision;
pub type HasMemberResponse = bool;
pub type MembersRevisionResponse = Revision;
pub type PrivilegesResponse = BTreeMap<SignerId, Vec<Capability>>;
pub type ProxyContractResponse = String;
pub type NonceResponse = Option<u64>;
