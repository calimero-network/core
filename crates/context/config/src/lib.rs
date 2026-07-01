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
#[derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub enum VisibilityMode {
    Open,
    Restricted,
}

/// A type-safe bitset of group member capabilities.
///
/// Replaces the bare `u32` that capability bitmasks used to flow as: a raw
/// integer silently accepted unknown/garbage bits and offered no way to tell a
/// capability mask apart from any other `u32`. The named flags are associated
/// constants of this type; combine them with `|`, test with
/// [`contains`](MemberCapabilities::contains), and cross wire/storage
/// boundaries with [`bits`](MemberCapabilities::bits).
///
/// Borsh/serde encode it as the underlying `u32`, byte-compatible with the
/// old representation. Deserialization preserves every bit, including ones
/// this build does not define: a newer peer may introduce a capability bit,
/// and rejecting it on the wire would make older peers fail to decode the op
/// and diverge from consensus. Validation is therefore a construction-time
/// choice, not a wire invariant —
/// [`from_bits`](MemberCapabilities::from_bits) rejects undefined bits (use it
/// for operator/API input you want to refuse),
/// [`from_bits_truncate`](MemberCapabilities::from_bits_truncate) drops them
/// when interpreting a stored/received mask, and the named-flag API only ever
/// tests defined bits.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct MemberCapabilities(u32);

impl MemberCapabilities {
    pub const CAN_CREATE_CONTEXT: Self = Self(1 << 0);
    pub const CAN_INVITE_MEMBERS: Self = Self(1 << 1);
    /// Permits a parent-group member to be inherited as a member of any
    /// `Open` subgroup beneath them (and transitively, any contexts those
    /// subgroups contain). Granted by default to non-admin members; admins
    /// revoke per-member as a deny-list when they want a specific user kept
    /// out of `Open` subgroups even though they remain in the parent.
    ///
    /// Reuses bit slot `1 << 2`, vacated by the prior `CAN_JOIN_OPEN_CONTEXTS`
    /// bit, which was never enforced anywhere and has been removed.
    pub const CAN_JOIN_OPEN_SUBGROUPS: Self = Self(1 << 2);
    pub const MANAGE_MEMBERS: Self = Self(1 << 3);
    pub const MANAGE_APPLICATION: Self = Self(1 << 4);
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
    pub const CAN_CREATE_SUBGROUP: Self = Self(1 << 5);
    /// Permits a non-admin namespace member to delete a subgroup (and its
    /// whole subtree) via the cascade-delete path. Checked on the namespace
    /// root, for the same determinism reason as [`Self::CAN_CREATE_SUBGROUP`].
    ///
    /// This is a delegation knob, not the default: an ordinary group admin
    /// does *not* get it implicitly here (and a later change tightens the
    /// baseline so even admins can't destroy a subtree they don't own — see
    /// the owner-gated-destruction work).
    pub const CAN_DELETE_SUBGROUP: Self = Self(1 << 6);
    /// Permits a member to flip a subgroup's [`VisibilityMode`]
    /// (`Open` ↔ `Restricted`) without holding full admin on it. The
    /// `SubgroupVisibilitySet` op is group-scoped (encrypted to the target
    /// subgroup's members), so this check is deterministic among exactly the
    /// peers that apply it — no root-level restriction needed.
    pub const CAN_MANAGE_VISIBILITY: Self = Self(1 << 7);
    /// Permits a member to set the `name` / `data` of the group, its members,
    /// or its contexts (the `*MetadataSet` ops) without holding full admin.
    /// Group admins hold this implicitly; a member may always set *their own*
    /// member metadata regardless of holding this bit. Like
    /// [`Self::CAN_MANAGE_VISIBILITY`], the `*MetadataSet` ops are
    /// group-scoped (encrypted to the target group's members), so this check
    /// is deterministic among exactly the peers that apply it — no root-level
    /// restriction needed.
    pub const CAN_MANAGE_METADATA: Self = Self(1 << 8);

    /// The union of every defined capability bit.
    pub const ALL: Self = Self(
        Self::CAN_CREATE_CONTEXT.bits()
            | Self::CAN_INVITE_MEMBERS.bits()
            | Self::CAN_JOIN_OPEN_SUBGROUPS.bits()
            | Self::MANAGE_MEMBERS.bits()
            | Self::MANAGE_APPLICATION.bits()
            | Self::CAN_CREATE_SUBGROUP.bits()
            | Self::CAN_DELETE_SUBGROUP.bits()
            | Self::CAN_MANAGE_VISIBILITY.bits()
            | Self::CAN_MANAGE_METADATA.bits(),
    );

    /// The empty capability set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// The raw bit representation (for wire/storage encoding).
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Construct from raw bits, returning `None` if any **undefined** bit is set.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Option<Self> {
        if bits & !Self::ALL.0 == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// Construct from raw bits, silently dropping any undefined bits.
    #[must_use]
    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits & Self::ALL.0)
    }

    /// Whether `self` contains every bit in `other`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no capability bit is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl core::ops::BitOr for MemberCapabilities {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for MemberCapabilities {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl core::ops::BitAnd for MemberCapabilities {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl core::ops::BitAndAssign for MemberCapabilities {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl core::ops::Sub for MemberCapabilities {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 & !rhs.0)
    }
}

// Wire/storage representation: the raw `u32`, byte-compatible with the bare
// `u32` these masks used to be. Unknown bits are preserved on the wire
// (forward-compatible with capabilities a newer peer may define); the
// named-flag API masks against the defined set when interpreting them.
impl borsh::BorshSerialize for MemberCapabilities {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        borsh::BorshSerialize::serialize(&self.0, writer)
    }
}

impl borsh::BorshDeserialize for MemberCapabilities {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        Ok(Self(u32::deserialize_reader(reader)?))
    }
}

impl serde::Serialize for MemberCapabilities {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for MemberCapabilities {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(u32::deserialize(deserializer)?))
    }
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
