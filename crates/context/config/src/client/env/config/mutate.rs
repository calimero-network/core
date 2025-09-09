use std::fmt::Debug;

use super::requests::{
    AddContextRequest, AddMembersRequest, GrantCapabilitiesRequest, RemoveMembersRequest,
    RevokeCapabilitiesRequest, UpdateApplicationRequest, UpdateProxyContractRequest,
};
use crate::client::env::utils;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::types::{Application, Capability, ContextId, ContextIdentity};
use crate::{ContextRequest, ContextRequestKind, RequestKind};

#[derive(Debug)]
pub struct ContextConfigMutate<'a, T> {
    pub client: CallClient<'a, T>,
}

#[derive(Debug)]
pub struct ContextConfigMutateRequest<'a, T> {
    client: CallClient<'a, T>,
    kind: RequestKind<'a>,
}

#[derive(Debug)]
struct Mutate<'a> {
    pub(crate) signing_key: [u8; 32],
    pub(crate) nonce: u64,
    pub(crate) kind: RequestKind<'a>,
}

// Protocol-specific implementations
// These modules contain the actual Method trait implementations for each blockchain protocol
#[cfg(feature = "ethereum_client")]
mod ethereum;
#[cfg(feature = "icp_client")]
mod icp;
#[cfg(feature = "near_client")]
mod near;
#[cfg(feature = "starknet_client")]
mod starknet;

impl<'a, T> ContextConfigMutate<'a, T> {
    pub fn add_context(
        self,
        context_id: ContextId,
        author_id: ContextIdentity,
        application: Application<'a>,
    ) -> ContextConfigMutateRequest<'a, T> {
        let add_request = AddContextRequest::new(context_id, author_id, application);
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: add_request.context_id,
                kind: ContextRequestKind::Add {
                    author_id: add_request.author_id,
                    application: add_request.application,
                },
            }),
        }
    }

    pub fn update_application(
        self,
        context_id: ContextId,
        application: Application<'a>,
    ) -> ContextConfigMutateRequest<'a, T> {
        let update_request = UpdateApplicationRequest::new(context_id, application);
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: update_request.context_id,
                kind: ContextRequestKind::UpdateApplication {
                    application: update_request.application,
                },
            }),
        }
    }

    pub fn add_members(
        self,
        context_id: ContextId,
        members: &'a [ContextIdentity],
    ) -> ContextConfigMutateRequest<'a, T> {
        let add_request = AddMembersRequest::new(context_id, members);
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: add_request.context_id,
                kind: ContextRequestKind::AddMembers {
                    members: add_request.members.into(),
                },
            }),
        }
    }

    pub fn remove_members(
        self,
        context_id: ContextId,
        members: &'a [ContextIdentity],
    ) -> ContextConfigMutateRequest<'a, T> {
        let remove_request = RemoveMembersRequest::new(context_id, members);
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: remove_request.context_id,
                kind: ContextRequestKind::RemoveMembers {
                    members: remove_request.members.into(),
                },
            }),
        }
    }

    pub fn grant(
        self,
        context_id: ContextId,
        capabilities: &'a [(ContextIdentity, Capability)],
    ) -> ContextConfigMutateRequest<'a, T> {
        let grant_request = GrantCapabilitiesRequest::new(context_id, capabilities);
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: grant_request.context_id,
                kind: ContextRequestKind::Grant {
                    capabilities: grant_request.capabilities.into(),
                },
            }),
        }
    }

    pub fn revoke(
        self,
        context_id: ContextId,
        capabilities: &'a [(ContextIdentity, Capability)],
    ) -> ContextConfigMutateRequest<'a, T> {
        let revoke_request = RevokeCapabilitiesRequest::new(context_id, capabilities);
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: revoke_request.context_id,
                kind: ContextRequestKind::Revoke {
                    capabilities: revoke_request.capabilities.into(),
                },
            }),
        }
    }

    pub fn update_proxy_contract(self, context_id: ContextId) -> ContextConfigMutateRequest<'a, T> {
        let update_request = UpdateProxyContractRequest::new(context_id);
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: update_request.context_id,
                kind: ContextRequestKind::UpdateProxyContract,
            }),
        }
    }
}

impl<'a, T: Transport> ContextConfigMutateRequest<'a, T> {
    pub async fn send(self, signing_key: [u8; 32], nonce: u64) -> Result<(), ClientError<T>> {
        let request = Mutate {
            signing_key,
            nonce,
            kind: self.kind,
        };

        utils::send(&self.client, Operation::Write(request)).await
    }
}
