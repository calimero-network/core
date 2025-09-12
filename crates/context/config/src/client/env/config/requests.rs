//! Request types for context configuration operations.
//!
//! This module contains all the request structs used for both querying and mutating
//! context configuration data across different blockchain protocols. These request types
//! are shared between the query and mutate clients to ensure consistency.

use core::ptr;

use serde::Serialize;

use crate::repr::Repr;
use crate::types::{Application, Capability, ContextId, ContextIdentity};

// ============================================================================
// Query Request Types
// ============================================================================

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

// ============================================================================
// Mutate Request Types
// ============================================================================

/// Request to add a new context.
#[derive(Debug)]
pub struct AddContextRequest<'a> {
    pub context_id: Repr<ContextId>,
    pub author_id: Repr<ContextIdentity>,
    pub application: Application<'a>,
}

impl<'a> AddContextRequest<'a> {
    pub fn new(
        context_id: ContextId,
        author_id: ContextIdentity,
        application: Application<'a>,
    ) -> Self {
        Self {
            context_id: Repr::new(context_id),
            author_id: Repr::new(author_id),
            application,
        }
    }
}

/// Request to update an application.
#[derive(Debug)]
pub struct UpdateApplicationRequest<'a> {
    pub context_id: Repr<ContextId>,
    pub application: Application<'a>,
}

impl<'a> UpdateApplicationRequest<'a> {
    pub fn new(context_id: ContextId, application: Application<'a>) -> Self {
        Self {
            context_id: Repr::new(context_id),
            application,
        }
    }
}

/// Request to add members to a context.
#[derive(Debug)]
pub struct AddMembersRequest<'a> {
    pub context_id: Repr<ContextId>,
    pub members: &'a [Repr<ContextIdentity>],
}

impl<'a> AddMembersRequest<'a> {
    pub fn new(context_id: ContextId, members: &'a [ContextIdentity]) -> Self {
        let members = unsafe {
            &*(ptr::from_ref::<[ContextIdentity]>(members) as *const [Repr<ContextIdentity>])
        };

        Self {
            context_id: Repr::new(context_id),
            members,
        }
    }
}

/// Request to remove members from a context.
#[derive(Debug)]
pub struct RemoveMembersRequest<'a> {
    pub context_id: Repr<ContextId>,
    pub members: &'a [Repr<ContextIdentity>],
}

impl<'a> RemoveMembersRequest<'a> {
    pub fn new(context_id: ContextId, members: &'a [ContextIdentity]) -> Self {
        let members = unsafe {
            &*(ptr::from_ref::<[ContextIdentity]>(members) as *const [Repr<ContextIdentity>])
        };

        Self {
            context_id: Repr::new(context_id),
            members,
        }
    }
}

/// Request to grant capabilities to members.
#[derive(Debug)]
pub struct GrantCapabilitiesRequest<'a> {
    pub context_id: Repr<ContextId>,
    pub capabilities: &'a [(Repr<ContextIdentity>, Capability)],
}

impl<'a> GrantCapabilitiesRequest<'a> {
    pub fn new(context_id: ContextId, capabilities: &'a [(ContextIdentity, Capability)]) -> Self {
        let capabilities = unsafe {
            &*(ptr::from_ref::<[(ContextIdentity, Capability)]>(capabilities)
                as *const [(Repr<ContextIdentity>, Capability)])
        };

        Self {
            context_id: Repr::new(context_id),
            capabilities,
        }
    }
}

/// Request to revoke capabilities from members.
#[derive(Debug)]
pub struct RevokeCapabilitiesRequest<'a> {
    pub context_id: Repr<ContextId>,
    pub capabilities: &'a [(Repr<ContextIdentity>, Capability)],
}

impl<'a> RevokeCapabilitiesRequest<'a> {
    pub fn new(context_id: ContextId, capabilities: &'a [(ContextIdentity, Capability)]) -> Self {
        let capabilities = unsafe {
            &*(ptr::from_ref::<[(ContextIdentity, Capability)]>(capabilities)
                as *const [(Repr<ContextIdentity>, Capability)])
        };

        Self {
            context_id: Repr::new(context_id),
            capabilities,
        }
    }
}

/// Request to update proxy contract.
#[derive(Debug, Copy, Clone)]
pub struct UpdateProxyContractRequest {
    pub context_id: Repr<ContextId>,
}

impl UpdateProxyContractRequest {
    pub fn new(context_id: ContextId) -> Self {
        Self {
            context_id: Repr::new(context_id),
        }
    }
}

// ============================================================================
// Re-exports for backward compatibility
// ============================================================================

// Re-export the RequestKind enum for use in mutate.rs
pub use crate::{ContextRequest, ContextRequestKind, RequestKind};
