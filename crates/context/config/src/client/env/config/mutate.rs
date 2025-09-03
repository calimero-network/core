use std::fmt::Debug;

use core::ptr;

use crate::client::env::utils;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::repr::Repr;
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
#[cfg(feature = "stellar_client")]
mod stellar;

impl<'a, T> ContextConfigMutate<'a, T> {
    pub fn add_context(
        self,
        context_id: ContextId,
        author_id: ContextIdentity,
        application: Application<'a>,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::Add {
                    author_id: Repr::new(author_id),
                    application,
                },
            }),
        }
    }

    pub fn update_application(
        self,
        context_id: ContextId,
        application: Application<'a>,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::UpdateApplication { application },
            }),
        }
    }

    pub fn add_members(
        self,
        context_id: ContextId,
        members: &[ContextIdentity],
    ) -> ContextConfigMutateRequest<'a, T> {
        let members = unsafe {
            &*(ptr::from_ref::<[ContextIdentity]>(members) as *const [Repr<ContextIdentity>])
        };

        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::AddMembers {
                    members: members.into(),
                },
            }),
        }
    }

    pub fn remove_members(
        self,
        context_id: ContextId,
        members: &[ContextIdentity],
    ) -> ContextConfigMutateRequest<'a, T> {
        let members = unsafe {
            &*(ptr::from_ref::<[ContextIdentity]>(members) as *const [Repr<ContextIdentity>])
        };

        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::RemoveMembers {
                    members: members.into(),
                },
            }),
        }
    }

    pub fn grant(
        self,
        context_id: ContextId,
        capabilities: &[(ContextIdentity, Capability)],
    ) -> ContextConfigMutateRequest<'a, T> {
        let capabilities = unsafe {
            &*(ptr::from_ref::<[(ContextIdentity, Capability)]>(capabilities)
                as *const [(Repr<ContextIdentity>, Capability)])
        };

        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::Grant {
                    capabilities: capabilities.into(),
                },
            }),
        }
    }

    pub fn revoke(
        self,
        context_id: ContextId,
        capabilities: &[(ContextIdentity, Capability)],
    ) -> ContextConfigMutateRequest<'a, T> {
        let capabilities = unsafe {
            &*(ptr::from_ref::<[(ContextIdentity, Capability)]>(capabilities)
                as *const [(Repr<ContextIdentity>, Capability)])
        };

        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::Revoke {
                    capabilities: capabilities.into(),
                },
            }),
        }
    }

    pub fn update_proxy_contract(self, context_id: ContextId) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
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
