use core::ptr;

use super::{ContextConfigMutate, ContextConfigMutateRequest};
use crate::repr::Repr;
use crate::types::{Application, Capability, ContextId, ContextIdentity};
use crate::{ContextRequest, ContextRequestKind, RequestKind};

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
                kind: ContextRequestKind::UpdateApplication {
                    application,
                    nonce: 0,
                },
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
                    nonce: 0,
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
                    nonce: 0,
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
                    nonce: 0,
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
                    nonce: 0,
                },
            }),
        }
    }
}
