#![allow(single_use_lifetimes, reason = "False positive")]

use serde::{Deserialize, Serialize};

pub mod repr;
pub mod types;

/// Inclusive bound on the namespace/subgroup parent-chain walk (group →
/// namespace root, and the subgroup-inheritance walk). The deepest legal
/// subgroup sits at depth `MAX_NAMESPACE_DEPTH`; a walk that observes the root's
/// `None` parent needs `MAX_NAMESPACE_DEPTH + 1` steps, so loops bound it with
/// `for _ in 0..=MAX_NAMESPACE_DEPTH`. Single source of truth shared by
/// `calimero-governance-store`, `calimero-context-client`, and `calimero-authz`,
/// which all walk the same parent chain.
pub const MAX_NAMESPACE_DEPTH: usize = 16;

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
    /// Permits this member to publish deltas attributed to ANOTHER member, under
    /// a warrant that member signed. Held by attested relays executing on a
    /// client's behalf.
    ///
    /// Deliberately not implied by admin, and never written as a row by the
    /// subgroup-admit cascade. It IS resolved through the membership anchor,
    /// which is a different thing and is the one exception to capabilities being
    /// read from the target group alone: a grant found on the ancestor a member
    /// inherits its membership through counts for the groups below it on that
    /// Open chain.
    ///
    /// That is a deliberate reversal of the original rule that only a row on the
    /// owning group counted. The rule was defensible in the abstract and broken
    /// in practice: a relay fleet is admitted once at the namespace root while
    /// contexts live in subgroups, so a namespace-wide grant authorized nothing
    /// at all, and there was no row to write for an inherited member short of
    /// admitting the relay directly to every subgroup. The scoping did not
    /// disappear, it moved to where the admin writes the grant — on the subgroup
    /// for one group, on the root for a fleet — and `Restricted` visibility and
    /// the per-member deny-list both still refuse, because a grant reaches
    /// wherever membership reaches and no further. The cost is real and is
    /// stated here rather than hidden: a root grant now also reaches Open
    /// subgroups created *after* it.
    ///
    /// Checked on the group that owns the target context and, failing that, on
    /// that group's membership anchor. Both are deterministic, unlike
    /// [`Self::CAN_CREATE_SUBGROUP`], and for a stronger reason than the owning
    /// group alone would give: resolving the membership already reads the
    /// anchor's own state — `check_path` consults `CAN_JOIN_OPEN_SUBGROUPS` in
    /// exactly the capability row this bit lives in to decide a non-admin's
    /// inheritance, and the parent's admin set to decide an admin's. A peer that
    /// cannot read the anchor cannot resolve the membership either, and would
    /// refuse the delta a step earlier. So the lookup needs no information a
    /// peer applying the delta could lack.
    pub const CAN_AUTHOR_ON_BEHALF: Self = Self(1 << 9);

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
            | Self::CAN_MANAGE_METADATA.bits()
            | Self::CAN_AUTHOR_ON_BEHALF.bits(),
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
// `u32` these masks used to be. Both Borsh (peer-to-peer op wire) and serde
// (JSON API surface) deserialize permissively — unknown bits are preserved,
// not rejected — so an older peer or handler can always decode a mask an
// newer version produced with an extra capability bit. Undefined bits are
// dropped at the point of interpretation, not decode: the `SetMemberCapabilities`
// / `SetDefaultCapabilities` handlers run the incoming value through
// `from_bits_truncate` before applying it, so a JSON caller sending `0xFFFFFFFF`
// never persists garbage bits. Use `from_bits` when you want to *reject* an
// undefined bit at construction instead of silently masking it.
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

#[cfg(test)]
mod capability_tests {
    use super::MemberCapabilities as C;

    /// Every named capability constant must be part of `ALL`. `from_bits`
    /// rejects any bit outside `ALL`, so a constant missing from the union
    /// would be silently rejected on the strict path and dropped on the
    /// truncating one. Adding a capability without adding it here (and to
    /// `ALL`) fails this test.
    #[test]
    fn all_is_the_union_of_every_named_capability() {
        let named = [
            C::CAN_CREATE_CONTEXT,
            C::CAN_INVITE_MEMBERS,
            C::CAN_JOIN_OPEN_SUBGROUPS,
            C::MANAGE_MEMBERS,
            C::MANAGE_APPLICATION,
            C::CAN_CREATE_SUBGROUP,
            C::CAN_DELETE_SUBGROUP,
            C::CAN_MANAGE_VISIBILITY,
            C::CAN_MANAGE_METADATA,
            C::CAN_AUTHOR_ON_BEHALF,
        ];

        let mut union = C::empty();
        for cap in named {
            assert!(
                C::ALL.contains(cap),
                "ALL is missing a named capability bit"
            );
            union |= cap;
        }
        assert_eq!(
            union,
            C::ALL,
            "ALL must equal the union of every named capability"
        );
    }

    /// `from_bits` accepts exactly the defined set and rejects any bit above it.
    #[test]
    fn from_bits_accepts_all_and_rejects_undefined() {
        assert_eq!(C::from_bits(C::ALL.bits()), Some(C::ALL));
        assert_eq!(C::from_bits(0), Some(C::empty()));
        assert_eq!(C::from_bits(C::ALL.bits() | (1 << 31)), None);
    }

    /// The wire/serde paths preserve undefined bits (forward-compat), while
    /// `from_bits_truncate` drops them at interpretation time.
    #[test]
    fn truncate_drops_undefined_bits() {
        let garbage = C::ALL.bits() | (1 << 30);
        assert_eq!(C::from_bits_truncate(garbage), C::ALL);
    }
}
