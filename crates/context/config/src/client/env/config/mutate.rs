use core::ptr;
use std::fmt::Debug;

use super::requests::{
    AddContextRequest, AddMembersRequest, CommitOpenInvitationRequest, GrantCapabilitiesRequest,
    RemoveMembersRequest, RevealOpenInvitationRequest, RevokeCapabilitiesRequest,
    UpdateApplicationRequest, UpdateProxyContractRequest,
};
use crate::client::env::utils;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::repr::Repr;
use crate::types::{
    AppKey, Application, BlockHeight, Capability, ContextGroupId, ContextId, ContextIdentity,
    SignedGroupRevealPayload, SignedRevealPayload, SignerId,
};
use crate::{
    ContextRequest, ContextRequestKind, GroupRequest, GroupRequestKind, RequestKind, VisibilityMode,
};

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
#[cfg(feature = "near_client")]
mod near;

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

    pub fn commit_invitation(
        self,
        context_id: ContextId,
        commitment_hash: String,
        expiration_block_height: BlockHeight,
    ) -> ContextConfigMutateRequest<'a, T> {
        let commit_open_invitation_request =
            CommitOpenInvitationRequest::new(context_id, commitment_hash, expiration_block_height);
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: commit_open_invitation_request.context_id,
                kind: ContextRequestKind::CommitOpenInvitation {
                    commitment_hash: commit_open_invitation_request.commitment_hash,
                    expiration_block_height: commit_open_invitation_request.expiration_block_height,
                },
            }),
        }
    }

    pub fn reveal_invitation(
        self,
        context_id: ContextId,
        payload: SignedRevealPayload,
    ) -> ContextConfigMutateRequest<'a, T> {
        let reveal_request = RevealOpenInvitationRequest::new(context_id, payload);
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: reveal_request.context_id,
                kind: ContextRequestKind::RevealOpenInvitation {
                    payload: reveal_request.payload,
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

    pub fn create_group(
        self,
        group_id: ContextGroupId,
        app_key: AppKey,
        target_application: Application<'a>,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::Create {
                    app_key: Repr::new(app_key),
                    target_application,
                },
            )),
        }
    }

    pub fn delete_group(self, group_id: ContextGroupId) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::Delete,
            )),
        }
    }

    pub fn add_group_members(
        self,
        group_id: ContextGroupId,
        members: &'a [SignerId],
    ) -> ContextConfigMutateRequest<'a, T> {
        // safety: `Repr<T>` is a transparent wrapper around `T`
        let members =
            unsafe { &*(ptr::from_ref::<[SignerId]>(members) as *const [Repr<SignerId>]) };
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::AddMembers {
                    members: members.into(),
                },
            )),
        }
    }

    pub fn remove_group_members(
        self,
        group_id: ContextGroupId,
        members: &'a [SignerId],
    ) -> ContextConfigMutateRequest<'a, T> {
        // safety: `Repr<T>` is a transparent wrapper around `T`
        let members =
            unsafe { &*(ptr::from_ref::<[SignerId]>(members) as *const [Repr<SignerId>]) };
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::RemoveMembers {
                    members: members.into(),
                },
            )),
        }
    }

    pub fn register_context_in_group(
        self,
        group_id: ContextGroupId,
        context_id: ContextId,
        visibility_mode: Option<VisibilityMode>,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::RegisterContext {
                    context_id: Repr::new(context_id),
                    visibility_mode,
                },
            )),
        }
    }

    pub fn unregister_context_from_group(
        self,
        group_id: ContextGroupId,
        context_id: ContextId,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::UnregisterContext {
                    context_id: Repr::new(context_id),
                },
            )),
        }
    }

    pub fn set_group_target(
        self,
        group_id: ContextGroupId,
        target_application: Application<'a>,
        migration_method: Option<String>,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::SetTargetApplication {
                    target_application,
                    migration_method,
                },
            )),
        }
    }

    pub fn commit_group_invitation(
        self,
        group_id: ContextGroupId,
        commitment_hash: String,
        expiration_block_height: BlockHeight,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::CommitGroupInvitation {
                    commitment_hash,
                    expiration_block_height,
                },
            )),
        }
    }

    pub fn join_context_via_group(
        self,
        group_id: ContextGroupId,
        context_id: ContextId,
        new_member: ContextIdentity,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::JoinContextViaGroup {
                    context_id: Repr::new(context_id),
                    new_member: Repr::new(new_member),
                },
            )),
        }
    }

    pub fn reveal_group_invitation(
        self,
        group_id: ContextGroupId,
        payload: SignedGroupRevealPayload,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::RevealGroupInvitation { payload },
            )),
        }
    }

    pub fn set_member_capabilities(
        self,
        group_id: ContextGroupId,
        member: SignerId,
        capabilities: u32,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::SetMemberCapabilities {
                    member: Repr::new(member),
                    capabilities,
                },
            )),
        }
    }

    pub fn set_context_visibility(
        self,
        group_id: ContextGroupId,
        context_id: ContextId,
        mode: VisibilityMode,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::SetContextVisibility {
                    context_id: Repr::new(context_id),
                    mode,
                },
            )),
        }
    }

    pub fn manage_context_allowlist(
        self,
        group_id: ContextGroupId,
        context_id: ContextId,
        add: &'a [SignerId],
        remove: &'a [SignerId],
    ) -> ContextConfigMutateRequest<'a, T> {
        // safety: `Repr<T>` is a transparent wrapper around `T`
        let add = unsafe { &*(ptr::from_ref::<[SignerId]>(add) as *const [Repr<SignerId>]) };
        let remove =
            unsafe { &*(ptr::from_ref::<[SignerId]>(remove) as *const [Repr<SignerId>]) };
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::ManageContextAllowlist {
                    context_id: Repr::new(context_id),
                    add: add.to_vec(),
                    remove: remove.to_vec(),
                },
            )),
        }
    }

    pub fn set_default_capabilities(
        self,
        group_id: ContextGroupId,
        default_capabilities: u32,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::SetDefaultCapabilities {
                    default_capabilities,
                },
            )),
        }
    }

    pub fn set_default_visibility(
        self,
        group_id: ContextGroupId,
        default_visibility: VisibilityMode,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Group(GroupRequest::new(
                Repr::new(group_id),
                GroupRequestKind::SetDefaultVisibility { default_visibility },
            )),
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
