#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
use core::{mem, ptr};
use std::collections::BTreeMap;

use serde::Serialize;

use crate::client::env::{utils, Method};
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::repr::{Repr, ReprTransmute};
use crate::types::{Application, Capability, ContextId, ContextIdentity, Revision, SignerId};

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

// Protocol-specific implementations
pub mod ethereum;
pub mod icp;
pub mod near;
pub mod starknet;
pub mod stellar;

// Re-export protocol-specific implementations
pub use ethereum::*;
pub use icp::*;
pub use near::*;
pub use starknet::*;
pub use stellar::*;
