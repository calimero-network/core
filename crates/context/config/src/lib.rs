#![allow(single_use_lifetimes, reason = "False positive")]

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

pub mod client_config;
pub mod repr;
pub mod types;

use repr::Repr;
use types::{
    AppKey, Application, Capability, ContextGroupId, ContextId, ContextIdentity,
    ExpirationTimestamp, SignedRevealPayload, SignerId,
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
        expiration_timestamp: ExpirationTimestamp,
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

/// Visibility mode for a subgroup within its parent group.
///
/// `Open`       — parent-group members are inherited as members of this subgroup
///                (and, transitively, of any contexts it contains), provided they
///                hold [`MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS`] at the
///                anchor parent.
/// `Restricted` — membership requires an explicit `add_group_members` call.
///
/// The walk in `check_group_membership` stops at the first `Restricted`
/// ancestor; a `Restricted` subgroup is a wall regardless of what sits above.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Permits a parent-group member to be inherited as a member of any
    /// `Open` subgroup beneath them (and transitively, any contexts those
    /// subgroups contain). Granted by default to non-admin members; admins
    /// revoke per-member as a deny-list when they want a specific user kept
    /// out of `Open` subgroups even though they remain in the parent.
    ///
    /// Reuses bit slot `1 << 2`, vacated by the prior `CAN_JOIN_OPEN_CONTEXTS`
    /// (never enforced — see issue #2256).
    pub const CAN_JOIN_OPEN_SUBGROUPS: u32 = 1 << 2;
    pub const MANAGE_MEMBERS: u32 = 1 << 3;
    pub const MANAGE_APPLICATION: u32 = 1 << 4;
    /// Permits a non-admin namespace member to create a subgroup **directly
    /// under the namespace root** (`parent_group_id == namespace_id`). This
    /// is the "any member can start a channel" primitive.
    ///
    /// Scoped to root-level subgroups on purpose: the apply-side
    /// authorization check in `execute_group_created` runs on every peer,
    /// and a peer can only verify the creator holds this bit if it can read
    /// the parent group's member-capability rows — which every namespace
    /// member can do for the root group (they all hold its key), but not
    /// necessarily for a deeper subgroup. Delegated creation of nested
    /// subgroups by non-admins is therefore left to a follow-up; namespace
    /// admins can still create subgroups at any depth.
    pub const CAN_CREATE_SUBGROUP: u32 = 1 << 5;
    /// Permits a non-admin namespace member to delete a subgroup (and its
    /// whole subtree) via the cascade-delete path. Checked on the namespace
    /// root, for the same determinism reason as [`Self::CAN_CREATE_SUBGROUP`].
    ///
    /// This is a delegation knob, not the default: an ordinary group admin
    /// does *not* get it implicitly here (and a later change tightens the
    /// baseline so even admins can't destroy a subtree they don't own — see
    /// the owner-gated-destruction work).
    pub const CAN_DELETE_SUBGROUP: u32 = 1 << 6;
    /// Permits a member to flip a subgroup's [`VisibilityMode`]
    /// (`Open` ↔ `Restricted`) without holding full admin on it. The
    /// `SubgroupVisibilitySet` op is group-scoped (encrypted to the target
    /// subgroup's members), so this check is deterministic among exactly the
    /// peers that apply it — no root-level restriction needed.
    pub const CAN_MANAGE_VISIBILITY: u32 = 1 << 7;
    /// Permits a member to set the `name` / `data` of the group, its members,
    /// or its contexts (the `*MetadataSet` ops) without holding full admin.
    /// Group admins hold this implicitly; a member may always set *their own*
    /// member metadata regardless of holding this bit. Like
    /// [`Self::CAN_MANAGE_VISIBILITY`], the `*MetadataSet` ops are
    /// group-scoped (encrypted to the target group's members), so this check
    /// is deterministic among exactly the peers that apply it — no root-level
    /// restriction needed.
    pub const CAN_MANAGE_METADATA: u32 = 1 << 8;
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
    /// Set the default capability bits for new members (admin-only).
    SetDefaultCapabilities {
        default_capabilities: u32,
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
