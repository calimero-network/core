#![allow(single_use_lifetimes, reason = "False positive")]

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

pub mod client_config;
pub mod repr;
pub mod types;

use repr::Repr;
use types::{
    AppKey, Application, BlockHeight, Capability, ContextGroupId, ContextId, ContextIdentity,
    SignedGroupRevealPayload, SignedRevealPayload, SignerId,
};

pub type Timestamp = u64;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Request<'a> {
    pub signer_id: Repr<SignerId>,
    pub nonce: u64,

    #[serde(borrow, flatten)]
    pub kind: RequestKind<'a>,
}

impl<'a> Request<'a> {
    #[must_use]
    pub fn new(signer_id: SignerId, kind: RequestKind<'a>, nonce: u64) -> Self {
        Request {
            signer_id: Repr::new(signer_id),
            kind,
            nonce,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum RequestKind<'a> {
    #[serde(borrow)]
    Context(ContextRequest<'a>),
    #[serde(borrow)]
    Group(GroupRequest<'a>),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ContextRequest<'a> {
    pub context_id: Repr<ContextId>,

    #[serde(borrow, flatten)]
    pub kind: ContextRequestKind<'a>,
}

impl<'a> ContextRequest<'a> {
    #[must_use]
    pub const fn new(context_id: Repr<ContextId>, kind: ContextRequestKind<'a>) -> Self {
        ContextRequest { context_id, kind }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[serde(deny_unknown_fields)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum ContextRequestKind<'a> {
    Add {
        author_id: Repr<ContextIdentity>,
        #[serde(borrow)]
        application: Application<'a>,
    },
    UpdateApplication {
        #[serde(borrow)]
        application: Application<'a>,
    },
    AddMembers {
        members: Cow<'a, [Repr<ContextIdentity>]>,
    },
    RemoveMembers {
        members: Cow<'a, [Repr<ContextIdentity>]>,
    },
    CommitOpenInvitation {
        commitment_hash: String,
        expiration_block_height: BlockHeight,
    },
    RevealOpenInvitation {
        payload: SignedRevealPayload,
    },
    Grant {
        capabilities: Cow<'a, [(Repr<ContextIdentity>, Capability)]>,
    },
    Revoke {
        capabilities: Cow<'a, [(Repr<ContextIdentity>, Capability)]>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GroupRequest<'a> {
    pub group_id: Repr<ContextGroupId>,

    #[serde(borrow, flatten)]
    pub kind: GroupRequestKind<'a>,
}

impl<'a> GroupRequest<'a> {
    #[must_use]
    pub const fn new(group_id: Repr<ContextGroupId>, kind: GroupRequestKind<'a>) -> Self {
        GroupRequest { group_id, kind }
    }
}

/// Visibility mode for a context within a group.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VisibilityMode {
    Open,
    Restricted,
}

/// Bitfield constants for group member capabilities.
#[derive(Copy, Clone, Debug)]
pub struct MemberCapabilities;

impl MemberCapabilities {
    pub const CAN_CREATE_CONTEXT: u32 = 1 << 0;
    pub const CAN_INVITE_MEMBERS: u32 = 1 << 1;
    pub const CAN_JOIN_OPEN_CONTEXTS: u32 = 1 << 2;
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[serde(deny_unknown_fields)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum GroupRequestKind<'a> {
    Create {
        app_key: Repr<AppKey>,
        #[serde(borrow)]
        target_application: Application<'a>,
    },
    Delete,
    AddMembers {
        members: Cow<'a, [Repr<SignerId>]>,
    },
    RemoveMembers {
        members: Cow<'a, [Repr<SignerId>]>,
    },
    RegisterContext {
        context_id: Repr<ContextId>,
        visibility_mode: Option<VisibilityMode>,
    },
    UnregisterContext {
        context_id: Repr<ContextId>,
    },
    SetTargetApplication {
        #[serde(borrow)]
        target_application: Application<'a>,
        migration_method: Option<String>,
    },
    /// Pre-approve a specific context to register via its proxy contract.
    /// Must be called by a group admin before the proxy path is exercised.
    ApproveContextRegistration {
        context_id: Repr<ContextId>,
    },
    CommitGroupInvitation {
        commitment_hash: String,
        expiration_block_height: BlockHeight,
    },
    RevealGroupInvitation {
        payload: SignedGroupRevealPayload,
    },
    /// Join a context within a group using group membership as authorization.
    /// Caller must be a group member; the context must belong to the group.
    JoinContextViaGroup {
        context_id: Repr<ContextId>,
        new_member: Repr<ContextIdentity>,
    },
    /// Set capability bits for a specific member (admin-only).
    SetMemberCapabilities {
        member: Repr<SignerId>,
        capabilities: u32,
    },
    /// Set visibility mode for a context (creator or admin).
    SetContextVisibility {
        context_id: Repr<ContextId>,
        mode: VisibilityMode,
    },
    /// Add/remove members from a context's allowlist (creator or admin).
    ManageContextAllowlist {
        context_id: Repr<ContextId>,
        add: Vec<Repr<SignerId>>,
        remove: Vec<Repr<SignerId>>,
    },
    /// Set the default capability bits for new members (admin-only).
    SetDefaultCapabilities {
        default_capabilities: u32,
    },
    /// Set the default visibility mode for new contexts (admin-only).
    SetDefaultVisibility {
        default_visibility: VisibilityMode,
    },
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[serde(deny_unknown_fields)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum SystemRequest {
    #[serde(rename_all = "camelCase")]
    SetValidityThreshold { threshold_ms: Timestamp },
}
