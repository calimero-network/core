use core::convert::Infallible;
use core::fmt::{self, Debug, Formatter};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::{AccountId, DeviceId};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId as PrimitiveContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey as PrimitivePublicKey;
use calimero_storage::logical_clock::HybridTimestamp;
use generic_array::sequence::Concat;
use generic_array::typenum::{U1, U32, U8};
use generic_array::GenericArray;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};
use zeroize::ZeroizeOnDrop;

// Group-key prefix allocation ledger. Every byte in `0x20..=0x4A` is taken
// except `0x25` and `0x2B` (retired, below); **the next free byte is `0x4B`**.
//
// The constants themselves are declared beside the key types they belong to
// rather than all in this block, which is why a ledger is needed at all: two
// families sharing a prefix would collide silently, and several of these keys are
// byte-identical in length, so the compiler cannot catch it either — a
// `GroupRevokedDevice` key and a `GroupDeviceBinding` key are both `[u8; 65]`,
// and only the prefix distinguishes them. Grep `u8 = 0x` in this file to
// re-derive this list before claiming a byte.
pub const GROUP_META_PREFIX: u8 = 0x20;
pub const GROUP_MEMBER_PREFIX: u8 = 0x21;
pub const GROUP_CONTEXT_INDEX_PREFIX: u8 = 0x22;
const CONTEXT_GROUP_REF_PREFIX: u8 = 0x23;
pub const GROUP_UPGRADE_PREFIX: u8 = 0x24;
/// Node-local fleet-convergence stamp for the group at `GROUP_UPGRADE_PREFIX`.
pub const GROUP_FLEET_COMPLETION_PREFIX: u8 = 0x4A;
pub const GROUP_MEMBER_CAPABILITY_PREFIX: u8 = 0x26;
pub const GROUP_DEFAULT_CAPS_PREFIX: u8 = 0x29;
pub const GROUP_SUBGROUP_VIS_PREFIX: u8 = 0x2A;
// 0x25 retired (was GROUP_SIGNING_KEY, a per-group cache of the node's one
// signing key; the subsystem was deleted, so nothing writes the row).
// 0x2B retired (was GROUP_CONTEXT_LAST_MIGRATION, pre-v2 migration markers).
// 0x2C retired (was GROUP_LOCAL_GOV_NONCE, the pre-window single-`u64`
// applied-nonce high-water mark).
/// Applied-nonce window per `(group_id, signer)` — contiguous floor plus the
/// sparse above-floor set — serialized as ONE value so it is written with a
/// single atomic `put` (no cross-key crash window). See
/// `calimero-governance-store::nonce_window`.
pub const GROUP_LOCAL_GOV_NONCE_WINDOW_PREFIX: u8 = 0x3C;
/// Per-group upgrade ladder: the ordered upgrade targets the group has moved
/// through, captured as fold state when an upgrade op advances
/// `GroupMeta.bytecode_id`. A behind context replays these rungs in order, each
/// in that release's own bytecode. (The context-resync marker lives in its own
/// `Column::ContextResyncRequested`, not in this group-prefix space.)
pub const GROUP_UPGRADE_LADDER_PREFIX: u8 = 0x3E;

#[derive(Clone, Copy, Debug)]
pub struct GroupPrefix;

impl KeyComponent for GroupPrefix {
    type LEN = U1;
}

#[derive(Clone, Copy, Debug)]
pub struct GroupIdComponent;

impl KeyComponent for GroupIdComponent {
    type LEN = U32;
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupMeta(Key<(GroupPrefix, GroupIdComponent)>);

impl GroupMeta {
    #[must_use]
    pub fn new(group_id: [u8; 32]) -> Self {
        Self(Key(
            GenericArray::from([GROUP_META_PREFIX]).concat(GenericArray::from(group_id))
        ))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

impl AsKeyParts for GroupMeta {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupMeta {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupMeta")
            .field("group_id", &self.group_id())
            .finish()
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupMember(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupMember {
    /// Keyed by the member's **account**, not its signing key.
    ///
    /// A membership row answers "may this PERSON act here", so one grant has to
    /// cover every device they hold. Both ids are 32 bytes, so the row layout is
    /// unchanged and confusing the two would still compile — which is exactly
    /// why the type says which one this is.
    #[must_use]
    pub fn new(group_id: [u8; 32], account: AccountId) -> Self {
        Self(Key(GenericArray::from([GROUP_MEMBER_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*account.as_bytes()))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn account(&self) -> AccountId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        AccountId::from(id)
    }
}

impl AsKeyParts for GroupMember {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupMember {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupMember {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupMember")
            .field("group_id", &self.group_id())
            .field("account", &self.account())
            .finish()
    }
}

/// The full applied-nonce window for a (group, signer) — see
/// `calimero-governance-store::nonce_window`. Holds a
/// [`GroupLocalGovNonceWindowValue`] written with a single atomic `put`.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupLocalGovNonceWindow(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupLocalGovNonceWindow {
    #[must_use]
    pub fn new(group_id: [u8; 32], signer: PrimitivePublicKey) -> Self {
        Self(Key(GenericArray::from([
            GROUP_LOCAL_GOV_NONCE_WINDOW_PREFIX,
        ])
        .concat(GenericArray::from(group_id))
        .concat(GenericArray::from(*signer))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn signer(&self) -> PrimitivePublicKey {
        let mut pk = [0; 32];
        pk.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        pk.into()
    }
}

impl AsKeyParts for GroupLocalGovNonceWindow {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupLocalGovNonceWindow {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupLocalGovNonceWindow {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupLocalGovNonceWindow")
            .field("group_id", &self.group_id())
            .field("signer", &self.signer())
            .finish()
    }
}

/// Value for [`GroupLocalGovNonceWindow`]: the contiguous applied-nonce floor
/// plus the sparse set of applied nonces above it. One value → one atomic write.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupLocalGovNonceWindowValue {
    pub floor: u64,
    pub above: Vec<u64>,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupContextIndex(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupContextIndex {
    #[must_use]
    pub fn new(group_id: [u8; 32], context_id: PrimitiveContextId) -> Self {
        Self(Key(GenericArray::from([GROUP_CONTEXT_INDEX_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*context_id))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        id.into()
    }
}

impl AsKeyParts for GroupContextIndex {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupContextIndex {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupContextIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupContextIndex")
            .field("group_id", &self.group_id())
            .field("context_id", &self.context_id())
            .finish()
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ContextGroupRef(Key<(GroupPrefix, GroupIdComponent)>);

impl ContextGroupRef {
    #[must_use]
    pub fn new(context_id: PrimitiveContextId) -> Self {
        Self(Key(
            GenericArray::from([CONTEXT_GROUP_REF_PREFIX]).concat(GenericArray::from(*context_id))
        ))
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id.into()
    }
}

impl AsKeyParts for ContextGroupRef {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for ContextGroupRef {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for ContextGroupRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextGroupRef")
            .field("context_id", &self.context_id())
            .finish()
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupUpgradeKey(Key<(GroupPrefix, GroupIdComponent)>);

impl GroupUpgradeKey {
    #[must_use]
    pub fn new(group_id: [u8; 32]) -> Self {
        Self(Key(
            GenericArray::from([GROUP_UPGRADE_PREFIX]).concat(GenericArray::from(group_id))
        ))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

impl AsKeyParts for GroupUpgradeKey {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupUpgradeKey {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupUpgradeKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupUpgradeKey")
            .field("group_id", &self.group_id())
            .finish()
    }
}

/// Unix timestamp when this node watched the whole cohort converge on the
/// group's [`GroupUpgradeValue::to_state_version`].
/// Key: `prefix(1) + group_id(32)` -> `u64`.
///
/// Node-local, and kept out of [`GroupUpgradeValue`] so that recording it can
/// never change that record's stored layout: this observation is written on an
/// observability path, long after the governance ops that write the record.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupFleetCompletion(Key<(GroupPrefix, GroupIdComponent)>);

impl GroupFleetCompletion {
    #[must_use]
    pub fn new(group_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([GROUP_FLEET_COMPLETION_PREFIX])
            .concat(GenericArray::from(group_id))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

impl AsKeyParts for GroupFleetCompletion {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupFleetCompletion {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupFleetCompletion {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupFleetCompletion")
            .field("group_id", &self.group_id())
            .finish()
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupUpgradeLadder(Key<(GroupPrefix, GroupIdComponent)>);

impl GroupUpgradeLadder {
    #[must_use]
    pub fn new(group_id: [u8; 32]) -> Self {
        Self(Key(
            GenericArray::from([GROUP_UPGRADE_LADDER_PREFIX]).concat(GenericArray::from(group_id))
        ))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

impl AsKeyParts for GroupUpgradeLadder {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupUpgradeLadder {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupUpgradeLadder {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupUpgradeLadder")
            .field("group_id", &self.group_id())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Group permission key types
// ---------------------------------------------------------------------------

/// Key for per-member capability bitfield: prefix + group_id + member account.
///
/// A capability is a grant to a person, so it is keyed the same way the
/// membership row it qualifies is — by [`AccountId`], covering every device
/// that account holds.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupMemberCapability(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupMemberCapability {
    #[must_use]
    pub fn new(group_id: [u8; 32], account: AccountId) -> Self {
        Self(Key(GenericArray::from([GROUP_MEMBER_CAPABILITY_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*account.as_bytes()))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn account(&self) -> AccountId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        AccountId::from(id)
    }
}

impl AsKeyParts for GroupMemberCapability {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupMemberCapability {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupMemberCapability {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupMemberCapability")
            .field("group_id", &self.group_id())
            .field("account", &self.account())
            .finish()
    }
}

/// Value for [`GroupMemberCapability`].
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupMemberCapabilityValue {
    pub capabilities: u32,
}

/// Key for group default capabilities: prefix + group_id.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupDefaultCaps(Key<(GroupPrefix, GroupIdComponent)>);

impl GroupDefaultCaps {
    #[must_use]
    pub fn new(group_id: [u8; 32]) -> Self {
        Self(Key(
            GenericArray::from([GROUP_DEFAULT_CAPS_PREFIX]).concat(GenericArray::from(group_id))
        ))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

impl AsKeyParts for GroupDefaultCaps {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupDefaultCaps {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupDefaultCaps {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupDefaultCaps")
            .field("group_id", &self.group_id())
            .finish()
    }
}

/// Value for [`GroupDefaultCaps`].
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupDefaultCapsValue {
    pub capabilities: u32,
}

/// Key for subgroup visibility: prefix + group_id.
///
/// Stores the [`crate::key::GroupSubgroupVisValue`] for a subgroup, which
/// governs whether parent-group members are inherited as members of this
/// subgroup (see `check_group_membership` in `calimero-context`).
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupSubgroupVis(Key<(GroupPrefix, GroupIdComponent)>);

impl GroupSubgroupVis {
    #[must_use]
    pub fn new(group_id: [u8; 32]) -> Self {
        Self(Key(
            GenericArray::from([GROUP_SUBGROUP_VIS_PREFIX]).concat(GenericArray::from(group_id))
        ))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

impl AsKeyParts for GroupSubgroupVis {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupSubgroupVis {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupSubgroupVis {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupSubgroupVis")
            .field("group_id", &self.group_id())
            .finish()
    }
}

/// Value for [`GroupSubgroupVis`]. `mode`: 0 = Open, 1 = Restricted.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupSubgroupVisValue {
    pub mode: u8,
}

pub const GROUP_CONTEXT_METADATA_PREFIX: u8 = 0x2F;

/// Stores the [`MetadataRecord`](calimero_primitives::metadata::MetadataRecord)
/// for a context registered within a group.
/// Key: prefix (1 byte) + group_id (32 bytes) + context_id (32 bytes) → `MetadataRecord`
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupContextMetadata(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupContextMetadata {
    #[must_use]
    pub fn new(group_id: [u8; 32], context_id: PrimitiveContextId) -> Self {
        Self(Key(GenericArray::from([GROUP_CONTEXT_METADATA_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*context_id))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..65]);
        id.into()
    }
}

impl AsKeyParts for GroupContextMetadata {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupContextMetadata {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupContextMetadata {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupContextMetadata")
            .field("group_id", &self.group_id())
            .field("context_id", &self.context_id())
            .finish()
    }
}

pub const GROUP_MEMBER_METADATA_PREFIX: u8 = 0x2D;

/// Stores the [`MetadataRecord`](calimero_primitives::metadata::MetadataRecord)
/// for a group member, scoped to a specific group.
/// Key: prefix (1 byte) + group_id (32 bytes) + member account (32 bytes) → `MetadataRecord`
///
/// Member metadata describes the person, so it follows the membership row's
/// keying: one record per [`AccountId`], not one per device.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupMemberMetadata(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupMemberMetadata {
    #[must_use]
    pub fn new(group_id: [u8; 32], member: AccountId) -> Self {
        Self(Key(GenericArray::from([GROUP_MEMBER_METADATA_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*member.as_bytes()))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn member(&self) -> AccountId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        AccountId::from(id)
    }
}

impl AsKeyParts for GroupMemberMetadata {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupMemberMetadata {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupMemberMetadata {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupMemberMetadata")
            .field("group_id", &self.group_id())
            .field("member", &self.member())
            .finish()
    }
}

pub const GROUP_METADATA_PREFIX: u8 = 0x2E;

/// Stores the [`MetadataRecord`](calimero_primitives::metadata::MetadataRecord)
/// for the group itself (a namespace is a root group, so this covers it).
/// Key: prefix (1 byte) + group_id (32 bytes) → `MetadataRecord`
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupMetadata(Key<(GroupPrefix, GroupIdComponent)>);

impl GroupMetadata {
    #[must_use]
    pub fn new(group_id: [u8; 32]) -> Self {
        Self(Key(
            GenericArray::from([GROUP_METADATA_PREFIX]).concat(GenericArray::from(group_id))
        ))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

impl AsKeyParts for GroupMetadata {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupMetadata {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupMetadata {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupMetadata")
            .field("group_id", &self.group_id())
            .finish()
    }
}

/// Sequence number component for op log entries (big-endian u64).
#[derive(Clone, Copy, Debug)]
pub struct SequenceComponent;

impl KeyComponent for SequenceComponent {
    type LEN = U8;
}

/// Prefix byte for the per-group op log (ordered by sequence number).
pub const GROUP_OP_LOG_PREFIX: u8 = 0x30;

/// Stores a `SignedGroupOp` (borsh bytes) keyed by `(group_id, sequence)`.
/// The sequence is a big-endian `u64` so entries sort lexicographically.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupOpLog(Key<(GroupPrefix, GroupIdComponent, SequenceComponent)>);

impl GroupOpLog {
    #[must_use]
    pub fn new(group_id: [u8; 32], sequence: u64) -> Self {
        Self(Key(GenericArray::from([GROUP_OP_LOG_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(sequence.to_be_bytes()))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 41]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn sequence(&self) -> u64 {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&AsRef::<[_; 41]>::as_ref(&self.0)[33..41]);
        u64::from_be_bytes(buf)
    }
}

impl AsKeyParts for GroupOpLog {
    type Components = (GroupPrefix, GroupIdComponent, SequenceComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupOpLog {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupOpLog {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupOpLog")
            .field("group_id", &self.group_id())
            .field("sequence", &self.sequence())
            .finish()
    }
}

/// Prefix byte for per-group head pointer (last op sequence + content hash).
pub const GROUP_OP_HEAD_PREFIX: u8 = 0x31;

pub const GROUP_MEMBER_CONTEXT_PREFIX: u8 = 0x32;
pub const GROUP_CONTEXT_MEMBER_CAP_PREFIX: u8 = 0x33;
pub const GROUP_PARENT_REF_PREFIX: u8 = 0x34;
pub const GROUP_CHILD_INDEX_PREFIX: u8 = 0x35;
/// Per-namespace (root group) node identity keypair.
pub const NAMESPACE_PARTICIPATION_PREFIX: u8 = 0x36;
/// Which service from a multi-service bundle a context runs.
/// Key: `prefix(1) + context_id(32)` → `ContextServiceNameValue`.
pub const CONTEXT_SERVICE_NAME_PREFIX: u8 = 0x37;

/// Stores the latest applied op sequence and content hash for a group.
/// Key: `prefix(1) + group_id(32)` → `GroupOpHeadValue`.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupOpHead(Key<(GroupPrefix, GroupIdComponent)>);

impl GroupOpHead {
    #[must_use]
    pub fn new(group_id: [u8; 32]) -> Self {
        Self(Key(
            GenericArray::from([GROUP_OP_HEAD_PREFIX]).concat(GenericArray::from(group_id))
        ))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

impl AsKeyParts for GroupOpHead {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupOpHead {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupOpHead {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupOpHead")
            .field("group_id", &self.group_id())
            .finish()
    }
}

/// Tracks which context memberships were granted through a group join.
/// Key: prefix + group_id + member account + context_id → context_identity bytes [u8; 32]
/// Used for cascade removal when a member is kicked from the group.
///
/// Keyed by [`AccountId`] because the cascade is driven by a membership
/// removal, which names an account; the *value* stays a context identity key,
/// since that is the per-device stamp the context itself was joined under.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupMemberContext(
    Key<(
        GroupPrefix,
        GroupIdComponent,
        GroupIdComponent,
        GroupIdComponent,
    )>,
);

impl GroupMemberContext {
    #[must_use]
    pub fn new(group_id: [u8; 32], member: AccountId, context_id: PrimitiveContextId) -> Self {
        Self(Key(GenericArray::from([GROUP_MEMBER_CONTEXT_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*member.as_bytes()))
            .concat(GenericArray::from(*context_id))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn member(&self) -> AccountId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[33..65]);
        AccountId::from(id)
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[65..97]);
        id.into()
    }
}

impl AsKeyParts for GroupMemberContext {
    type Components = (
        GroupPrefix,
        GroupIdComponent,
        GroupIdComponent,
        GroupIdComponent,
    );

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupMemberContext {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupMemberContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupMemberContext")
            .field("group_id", &self.group_id())
            .field("member", &self.member())
            .field("context_id", &self.context_id())
            .finish()
    }
}

/// Per-context per-member capability bitfield.
/// Key: prefix(1) + group_id(32) + context_id(32) + member account(32) = 97 bytes
/// Value: u8 (capability bitfield)
///
/// Account-keyed for the same reason as [`GroupMemberCapability`]: a grant is
/// made to a person and must hold for every device they act from.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupContextMemberCap(
    Key<(
        GroupPrefix,
        GroupIdComponent,
        GroupIdComponent,
        GroupIdComponent,
    )>,
);

impl GroupContextMemberCap {
    #[must_use]
    pub fn new(group_id: [u8; 32], context_id: PrimitiveContextId, member: AccountId) -> Self {
        Self(Key(GenericArray::from([GROUP_CONTEXT_MEMBER_CAP_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*context_id))
            .concat(GenericArray::from(*member.as_bytes()))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[33..65]);
        id.into()
    }

    #[must_use]
    pub fn member(&self) -> AccountId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[65..]);
        AccountId::from(id)
    }
}

impl AsKeyParts for GroupContextMemberCap {
    type Components = (
        GroupPrefix,
        GroupIdComponent,
        GroupIdComponent,
        GroupIdComponent,
    );

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupContextMemberCap {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupContextMemberCap {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupContextMemberCap")
            .field("group_id", &self.group_id())
            .field("context_id", &self.context_id())
            .field("member", &self.member())
            .finish()
    }
}

/// Value for [`GroupOpHead`].
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupOpHeadValue {
    pub sequence: u64,
    pub dag_heads: Vec<[u8; 32]>,
}

/// Stored against [`GroupMeta`]. Captures the immutable + mutable metadata of a
/// context group.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupMetaValue {
    pub bytecode_id: [u8; 32],
    pub target_application_id: ApplicationId,
    pub created_at: u64,
    /// The founding admin, named by **account**. Both this and
    /// `owner_identity` are principals — they answer "who may act" — so they
    /// key the same way every other governance gate does.
    pub admin_identity: AccountId,
    /// Single-instance Owner of this group. Distinct from the legacy
    /// `admin_identity` (which is now a fallback creator-admin marker for
    /// pre-existing groups). The Owner has exclusive privileges no other
    /// admin can perform: `TransferOwnership`, `DeleteGroup`/
    /// `DeleteNamespace`, and immunity from involuntary `MemberRemoved`.
    ///
    /// Set to the account of the signer of `CreateGroupRequest` on group
    /// creation. New groups have `owner_identity == admin_identity` initially.
    /// Transferable via `GroupOp::TransferOwnership { new_owner }`.
    pub owner_identity: AccountId,
    pub migration: Option<Vec<u8>>,
    /// When true, joining members auto-subscribe to all visible contexts.
    pub auto_join: bool,
}

/// Per-member opt-in flags that drive the auto-follow handler.
///
/// - `contexts`: when the group gets a new [`GroupOp::ContextRegistered`],
///   the auto-follow handler emits a `JoinContext` on behalf of this member.
/// - `subgroups`: when a subgroup is nested under a group where this member
///   is present, the handler emits a self-admission op in the child carrying
///   the member's inherited role.
///
/// # Default
///
/// `contexts = true`, `subgroups = false`. Joining a group implies wanting
/// the group's data: the user mental model on the join flow is "I'm in this
/// namespace, I want its contexts." The opt-out is an explicit
/// `set_member_auto_follow(contexts: false)` per member, per group. This
/// closes #2422 Option 1 (joiners weren't auto-followed by default,
/// producing the "I joined but no data syncs" UX bug).
///
/// `subgroups` stays `false` because subgroup auto-follow for non-TEE roles
/// requires a new admission op (existing `MemberAdded` must be admin-signed —
/// see `crates/context/src/auto_follow.rs` module doc). The TEE fleet-join
/// path overrides both to `true` via an explicit `SetMemberAutoFollow` op
/// after admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct AutoFollowFlags {
    pub contexts: bool,
    pub subgroups: bool,
}

impl Default for AutoFollowFlags {
    fn default() -> Self {
        Self {
            contexts: true,
            subgroups: false,
        }
    }
}

/// Stored against [`GroupMember`]. Tracks the member's role and, for the local
/// node, the Ed25519 key pair used for sync key-share across all contexts in
/// this group.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupMemberValue {
    pub role: GroupMemberRole,
    pub private_key: Option<[u8; 32]>,
    pub sender_key: Option<[u8; 32]>,
    pub auto_follow: AutoFollowFlags,
}

/// Tracks the progress of a group-wide upgrade operation.
/// Stored against [`GroupUpgradeKey`].
///
/// `ApplicationId` is stable across versions (`hash(package, signer_id)`), so
/// upgrades are tracked by semver version string from the local
/// `ApplicationMeta`, not by application id.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupUpgradeValue {
    /// Semver version of the application before the upgrade, read from the
    /// current application's `ApplicationMeta.version`.
    pub from_version: String,
    /// Semver version of the target application, read from the target
    /// application's `ApplicationMeta.version`.
    pub to_version: String,
    pub migration: Option<Vec<u8>>,
    pub initiated_at: u64,
    pub initiated_by: PrimitivePublicKey,
    pub status: GroupUpgradeStatus,
    /// Sticky cascade fence boundary: the HLC the originating `CascadeUpgrade`
    /// op was stamped with, identical on every node that applied it. `None` for
    /// non-cascade upgrades. NEVER cleared once set (survives `Completed`) —
    /// the boundary the state-delta HLC fence reads.
    pub cascade_hlc: Option<HybridTimestamp>,
    /// The migration's expand-entry governance position: the
    /// `NamespaceGovHead.sequence` captured when this cascade was applied.
    /// Unlike [`Self::cascade_hlc`] (an NTP64 physical-time HLC), this is a
    /// monotonic governance-op counter — the SAME number space the migration
    /// heartbeat's `synced_up_to_hlc` (`= head.sequence`) lives in, so the
    /// migration-status rollup pins the cohort by comparing `synced_up_to_hlc <
    /// cascade_seq` like-for-like. `None` for non-cascade upgrades.
    pub cascade_seq: Option<u64>,
    /// ABI state version of the target application, from its embedded schema.
    /// The migration rollup compares each member's loaded state version against
    /// this. `0` means the target's ABI was unreadable, which the rollup treats
    /// as an unsatisfiable target rather than a satisfied one.
    pub to_state_version: u32,
}

/// One rung of a group's upgrade ladder: the bytecode blob (`bytecode_id`), the
/// application id an upgrade op targeted, and that release's registry
/// coordinates - carried per rung because a bundle's application id is
/// version-stable and so names every rung equally. Stored in
/// causal-application order inside [`UpgradeLadderValue`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct LadderRung {
    pub bytecode_id: [u8; 32],
    pub application_id: ApplicationId,
    pub package: String,
    pub version: String,
}

/// Stored against [`GroupUpgradeLadder`]. Append-only fold state: every op
/// that advances `GroupMeta.bytecode_id` appends its target here, so a context
/// behind the group can replay the exact sequence of upgrades — each rung in
/// that release's own bytecode.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct UpgradeLadderValue {
    pub rungs: Vec<LadderRung>,
}

/// State machine for a group upgrade operation.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub enum GroupUpgradeStatus {
    InProgress {
        total: u32,
        completed: u32,
        failed: u32,
    },
    Completed {
        /// Unix timestamp when the last context was upgraded, or `None` when
        /// each context self-migrates independently without coordination.
        /// NODE-LOCAL: it says nothing about the rest of the cohort, which
        /// [`GroupFleetCompletion`] answers.
        completed_at: Option<u64>,
    },
}

/// Maps a child group to its parent group.
/// Key: `prefix(1) + child_group_id(32)` -> `[u8; 32]` (parent group ID).
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupParentRef(Key<(GroupPrefix, GroupIdComponent)>);

impl GroupParentRef {
    #[must_use]
    pub fn new(child_group_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([GROUP_PARENT_REF_PREFIX])
            .concat(GenericArray::from(child_group_id))))
    }

    #[must_use]
    pub fn child_group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

impl AsKeyParts for GroupParentRef {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupParentRef {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupParentRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupParentRef")
            .field("child_group_id", &self.child_group_id())
            .finish()
    }
}

/// Reverse index: parent_group_id + child_group_id -> unit.
/// Allows listing all children of a group.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupChildIndex(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupChildIndex {
    #[must_use]
    pub fn new(parent_group_id: [u8; 32], child_group_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([GROUP_CHILD_INDEX_PREFIX])
            .concat(GenericArray::from(parent_group_id))
            .concat(GenericArray::from(child_group_id))))
    }

    #[must_use]
    pub fn parent_group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn child_group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        id
    }
}

impl AsKeyParts for GroupChildIndex {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupChildIndex {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupChildIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupChildIndex")
            .field("parent_group_id", &self.parent_group_id())
            .field("child_group_id", &self.child_group_id())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Namespace participation: which namespaces this node takes part in
// ---------------------------------------------------------------------------

/// Store key marking that this node participates in a namespace (root group).
/// Key layout: `NAMESPACE_PARTICIPATION_PREFIX (1 byte) + namespace_id (32 bytes)`.
/// The namespace_id is the root group's ContextGroupId.
///
/// This row used to hold a per-namespace keypair, and enumerating it answered two
/// questions at once: what do I sign with here, and which namespaces am I in. The
/// signing key is now node-level ([`NodeIdentity`]), but the second question is
/// still real — `join_context` syncs exactly the namespaces this node takes part
/// in, and the startup buffered-op sweep walks the same set — so the row stays as
/// the index it always also was, carrying no key material.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NamespaceParticipation(Key<(GroupPrefix, GroupIdComponent)>);

impl NamespaceParticipation {
    #[must_use]
    pub fn new(namespace_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([NAMESPACE_PARTICIPATION_PREFIX])
            .concat(GenericArray::from(namespace_id))))
    }

    #[must_use]
    pub fn namespace_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

/// Prefix for the cold-start inviter hint. NODE-LOCAL, never gossiped.
pub const NAMESPACE_BOOTSTRAP_INVITER_PREFIX: u8 = 0x47;

// ---------------------------------------------------------------------------
// Bootstrap inviter: the cold-start beacon-verification hint
// ---------------------------------------------------------------------------

/// The account an invitation CLAIMED its inviter acts as, kept only until
/// genesis says who the admin really is.
///
/// Key layout: `NAMESPACE_BOOTSTRAP_INVITER_PREFIX (1 byte) + namespace_id (32)`.
///
/// Its own row because the value is a HINT and everywhere else it could live is
/// authority. It arrives in the unsigned half of an invitation, so anything
/// relaying one can choose it; put in `GroupMetaValue::admin_identity` it became
/// the permanent trust root, since genesis refuses to overwrite a non-placeholder
/// admin, and `owner_identity` is no better — that one gates ownership transfer
/// and group deletion.
///
/// Read only while the namespace has no established admin, and consulted for one
/// thing: letting the inviter's readiness beacons verify before any DAG has
/// arrived, which is the trigger a cold-start joiner needs to pull governance at
/// all. A wrong value there costs a bogus beacon accepted in that window and
/// nothing after it.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NamespaceBootstrapInviter(Key<(GroupPrefix, GroupIdComponent)>);

impl NamespaceBootstrapInviter {
    #[must_use]
    pub fn new(namespace_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([
            NAMESPACE_BOOTSTRAP_INVITER_PREFIX,
        ])
        .concat(GenericArray::from(namespace_id))))
    }
}

impl AsKeyParts for NamespaceBootstrapInviter {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for NamespaceBootstrapInviter {
    type Error = ();

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl AsKeyParts for NamespaceParticipation {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for NamespaceParticipation {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for NamespaceParticipation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("NamespaceParticipation")
            .field("namespace_id", &self.namespace_id())
            .finish()
    }
}

/// Value for [`NamespaceParticipation`]. The Ed25519 keypair this node uses as its
/// member identity within the namespace, plus a sender key for encrypted sync.
///
/// Zeroized on drop, for the same reason the two node-local account secrets in
/// this file are: this row holds two live secrets, and a plain drop leaves them in
/// freed heap for whatever reads that page next — a core dump, a swap file, or the
/// next allocation. `Copy` is therefore also off the table: it would duplicate the
/// secret implicitly on every read, and the wipe only ever reaches the original.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NamespaceParticipationValue {
    /// Reserved. The row is a set membership marker — its presence is the whole
    /// meaning — but borsh needs a field, and a `u8` leaves room to record
    /// *when* or *how* the node joined without a second key family later.
    pub reserved: u8,
}

// ---------------------------------------------------------------------------
// Context service name (multi-service bundles)
// ---------------------------------------------------------------------------

/// Stores which service from a multi-service bundle a context runs.
/// Written during `ContextRegistered` governance application so joining
/// nodes know the service_name before `ContextMeta` is created.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ContextServiceName(Key<(GroupPrefix, GroupIdComponent)>);

impl ContextServiceName {
    #[must_use]
    pub fn new(context_id: PrimitiveContextId) -> Self {
        Self(Key(GenericArray::from([CONTEXT_SERVICE_NAME_PREFIX])
            .concat(GenericArray::from(*context_id))))
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id.into()
    }
}

impl AsKeyParts for ContextServiceName {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for ContextServiceName {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for ContextServiceName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextServiceName")
            .field("context_id", &self.context_id())
            .finish()
    }
}

/// Value stored for a context service name.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ContextServiceNameValue {
    pub service_name: Box<str>,
}

// ---------------------------------------------------------------------------
// Namespace governance op storage
// ---------------------------------------------------------------------------

/// Prefix for namespace governance op entries.
pub const NAMESPACE_GOV_OP_PREFIX: u8 = 0x38;

/// Prefix for namespace governance DAG head entries.
pub const NAMESPACE_GOV_HEAD_PREFIX: u8 = 0x39;

/// Stores a namespace governance op (full decrypted or opaque skeleton).
/// Key layout: `prefix(1) + namespace_id(32) + delta_id(32)`.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NamespaceGovOp(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl NamespaceGovOp {
    #[must_use]
    pub fn new(namespace_id: [u8; 32], delta_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([NAMESPACE_GOV_OP_PREFIX])
            .concat(GenericArray::from(namespace_id))
            .concat(GenericArray::from(delta_id))))
    }

    #[must_use]
    pub fn namespace_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn delta_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..65]);
        id
    }

    /// Is this row actually a namespace gov-op row?
    ///
    /// [`GroupDeviceBinding`] has the IDENTICAL key layout — same prefix width,
    /// same two 32-byte components — so a walk that seeks into this family and
    /// stops only on a key that fails to *parse* runs straight into the binding
    /// rows, whose group id sits in the same bytes this type reads as the
    /// namespace id. For a namespace root those ids are equal, so the walk
    /// accepts the row and decodes a binding value as op bytes: "Not all bytes
    /// read". Bound such a walk on this predicate, never on width plus id.
    #[must_use]
    pub fn is_gov_op_row(&self) -> bool {
        AsRef::<[_; 65]>::as_ref(&self.0)[0] == NAMESPACE_GOV_OP_PREFIX
    }
}

impl AsKeyParts for NamespaceGovOp {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for NamespaceGovOp {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for NamespaceGovOp {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("NamespaceGovOp")
            .field("namespace_id", &self.namespace_id())
            .field("delta_id", &self.delta_id())
            .finish()
    }
}

/// Value for [`NamespaceGovOp`]. Contains borsh-encoded skeleton or full op.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NamespaceGovOpValue {
    pub skeleton_bytes: Vec<u8>,
}

/// Stores the current namespace governance DAG heads.
/// Key layout: `prefix(1) + namespace_id(32)`.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NamespaceGovHead(Key<(GroupPrefix, GroupIdComponent)>);

impl NamespaceGovHead {
    #[must_use]
    pub fn new(namespace_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([NAMESPACE_GOV_HEAD_PREFIX])
            .concat(GenericArray::from(namespace_id))))
    }

    #[must_use]
    pub fn namespace_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

impl AsKeyParts for NamespaceGovHead {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for NamespaceGovHead {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for NamespaceGovHead {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("NamespaceGovHead")
            .field("namespace_id", &self.namespace_id())
            .finish()
    }
}

/// Value for [`NamespaceGovHead`].
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NamespaceGovHeadValue {
    pub sequence: u64,
    pub dag_heads: Vec<[u8; 32]>,
}

// ---------------------------------------------------------------------------
// Group key storage (envelope-based key management)
// ---------------------------------------------------------------------------

/// Prefix for group key entries.
pub const GROUP_KEY_PREFIX: u8 = 0x3A;

/// Prefix for the per-group deny-list. An entry under
/// `(group_id, member_pubkey)` means the member is currently denied at the
/// network/topic layer — state deltas they sign are dropped before reaching
/// the cross-DAG check. Populated on `MemberRemoved` / `MemberLeft` /
/// equivalent removal apply; cleared on `MemberAdded` /
/// `MemberJoinedViaTeeAttestation` apply for the same member (handles
/// re-add). Per-group rather than per-peer-id because the same identity
/// can be a member of multiple groups; denying their connection wholesale
/// would drop legitimate traffic for groups they still belong to.
pub const GROUP_DENIED_MEMBER_PREFIX: u8 = 0x3B;

/// Prefix for the namespace-root *inherited* deny-list. An entry under
/// `(namespace_root_id, member_pubkey)` means the member lost their root
/// membership (evicted from / left the namespace root) and so lost every
/// Open-subgroup membership they held purely by INHERITANCE from that root row.
/// Their state deltas to any descendant subgroup's contexts are dropped at the
/// receive filter — which resolves a context's group to its root and checks
/// here — until they are re-admitted at the root.
///
/// Separate column from `GROUP_DENIED_MEMBER_PREFIX` on purpose: that one is the
/// per-group "not a member" view carrying the "never coexists with a direct
/// member row" invariant; this one is a receive-filter-only view keyed to the
/// root, whose clear lifecycle is the root re-admission (not the per-group row
/// write). Populated on `MemberRemoved` / `MemberLeft` at the namespace root;
/// cleared when a direct root row is re-written (`add_member_with_keys`, which
/// every root re-admission funnels through). Hash-neutral, like the direct
/// deny-list.
pub const GROUP_INHERITED_DENIED_MEMBER_PREFIX: u8 = 0x40;

/// Device→account bindings (see [`GroupDeviceBinding`]).
pub const GROUP_DEVICE_BINDING_PREFIX: u8 = 0x41;

/// Revocation tombstones for devices (see [`GroupRevokedDevice`]).
pub const GROUP_REVOKED_DEVICE_PREFIX: u8 = 0x42;

/// Per-account current root key (see [`GroupAccountKey`]).
pub const GROUP_ACCOUNT_KEY_PREFIX: u8 = 0x43;

/// This node's own device identity (see [`NodeDeviceIdentity`]) — one per node,
/// not one per namespace.
pub const NODE_DEVICE_IDENTITY_PREFIX: u8 = 0x44;

/// This node's account root secret (see [`NodeAccountRoot`]).
pub const NODE_ACCOUNT_ROOT_PREFIX: u8 = 0x45;

/// This node's signing identity (see [`NodeIdentity`]).
pub const NODE_IDENTITY_PREFIX: u8 = 0x48;

/// This node's signing identity — a **singleton**, keyed by nothing but its own
/// prefix (see [`NODE_IDENTITY_PREFIX`]).
///
/// Node-level rather than per-namespace. The key a node signs with is recorded as
/// its device's `sign_pk`, and a device is one installation, not one installation
/// per scope — so a key that varied by namespace would be a certificate claiming a
/// key that only sometimes signs. It also bought nothing: a per-namespace key is
/// published in every namespace's device binding, so it correlates a person across
/// namespaces exactly as a shared one does.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NodeIdentity(Key<(GroupPrefix,)>);

impl NodeIdentity {
    #[must_use]
    pub fn new() -> Self {
        Self(Key(GenericArray::from([NODE_IDENTITY_PREFIX])))
    }
}

impl Default for NodeIdentity {
    fn default() -> Self {
        Self::new()
    }
}

impl AsKeyParts for NodeIdentity {
    type Components = (GroupPrefix,);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for NodeIdentity {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for NodeIdentity {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NodeIdentity").finish()
    }
}

/// The keypair behind [`NodeIdentity`].
#[derive(Clone, ZeroizeOnDrop)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NodeIdentityValue {
    pub public_key: [u8; 32],
    pub private_key: [u8; 32],
}

/// Redacted by hand, never derived. `private_key` is what this node signs every
/// governance op and state delta with; a derived `Debug` puts it one `tracing`
/// field or one error context away from a log file.
impl Debug for NodeIdentityValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeIdentityValue")
            .field("public_key", &self.public_key)
            .field("private_key", &"[redacted]")
            .finish()
    }
}

/// A member key that has endorsed an account into a group (see
/// [`GroupAccountEndorser`]).
pub const GROUP_ACCOUNT_ENDORSER_PREFIX: u8 = 0x46;

/// Prefix for the pending-key-rotation worklist. A row marks: `group_id` still
/// owes a forward-secrecy key rotation because `departed` left, and no rotation
/// has landed yet.
///
/// Written by the `MemberLeft` apply — which is deterministic and replicated, so
/// every node derives the SAME worklist with no coordination — and cleared by the
/// `GroupKeyRotated` apply that carries the new key. A leaver cannot rotate for
/// themselves (they would have to mint the key they are being cut off from, and
/// peers reject a rotation from a non-admin anyway), so the row is the durable
/// hand-off that lets a remaining admin finish the job, including after a restart.
pub const GROUP_PENDING_KEY_ROTATION_PREFIX: u8 = 0x3F;

/// Prefix for the per-group re-entry block. An entry under
/// `(group_id, identity)` means the identity has EXITED the group and may not
/// re-enter it passively — the value records how they left, which decides what
/// can readmit them.
///
/// Distinct from the deny-list (`GROUP_DENIED_MEMBER_PREFIX`) in both lifetime
/// and purpose, and the two must not be conflated. The deny-list is a derived
/// view of "not currently a member" that silences an identity's traffic at the
/// receive filter, and it is retracted the moment a member row is written. This
/// block is an *authorization* record that deliberately SURVIVES a member-row
/// write, because its whole job is to make a re-join attempt fail. Writing a
/// member row must never clear it; only the paths named below may.
///
/// Written on `MemberRemoved` / `MemberLeft` apply. Cleared on `MemberAdded`
/// apply (an admin re-adding you is the only unban) and, for a `Left` block
/// only, by a successful invitation join with a nonce this identity has not
/// already consumed.
pub const GROUP_REENTRY_BLOCK_PREFIX: u8 = 0x27;

/// Prefix for consumed invitations, keyed
/// `(group_id, identity, invitation_nonce)`. An entry means this identity has
/// already used this specific invitation to join this group, so presenting it
/// again cannot readmit them — they need a freshly issued one.
///
/// Keyed by identity as well as nonce on purpose: an open invitation is a
/// bearer token with no invitee field, so the same nonce legitimately admits
/// many *different* identities (that is what makes a shared join link work).
/// What must not happen is the same identity replaying it after they exit.
/// Consumption is therefore per-identity, not global.
pub const GROUP_CONSUMED_INVITATION_PREFIX: u8 = 0x28;

/// Prefix for the durable pending-self-purge marker. A row keyed by
/// `namespace_id` marks that THIS node was confirmed TEE-self-evicted from
/// the namespace and the local-state cascade purge is in flight or
/// incomplete. Written by the self-purge listener (`calimero-context`'s
/// `self_purge`) at dispatch time — BEFORE the cascade runs — only when the
/// removal was a role-scoped `TeeMemberRemoved` targeting this node's
/// identity; cleared once the cascade fully completes (signing keys gone).
/// The startup reconcile sweep completes ONLY marked namespaces, so it
/// cannot false-purge a pending-join or a non-TEE soft-leave (#2721).
pub const PENDING_SELF_PURGE_PREFIX: u8 = 0x3D;

/// Stores a group encryption key by `(group_id, key_id)`.
///
/// Key layout: `prefix(1) + group_id(32) + key_id(32)` = 65 bytes.
/// `key_id` is `sha256(group_key)` — a content-addressed identifier that
/// appears on every encrypted governance op and state delta so receivers
/// know which key to use for decryption.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupKeyEntry(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupKeyEntry {
    #[must_use]
    pub fn new(group_id: [u8; 32], key_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([GROUP_KEY_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(key_id))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn key_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..65]);
        id
    }

    /// Is this row actually a group-key row?
    ///
    /// A typed iterator over [`Column::Group`] keeps yielding rows once it walks
    /// past this family, and it decodes each one as whatever type the caller
    /// asked for. Several neighbouring families are also 65 bytes wide and also
    /// carry the group id in bytes 1..33 — the device binding (`0x41`) and the
    /// account endorser (`0x46`), both of which sort AFTER `0x3A` — so
    /// [`Self::group_id`] answers plausibly for a row that is not a key at all.
    /// Any scan that seeks into this family must stop on this predicate rather
    /// than on the group id.
    #[must_use]
    pub fn is_group_key_row(&self) -> bool {
        AsRef::<[_; 65]>::as_ref(&self.0)[0] == GROUP_KEY_PREFIX
    }
}

impl AsKeyParts for GroupKeyEntry {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupKeyEntry {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupKeyEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupKeyEntry")
            .field("group_id", &self.group_id())
            .field("key_id", &self.key_id())
            .finish()
    }
}

/// Per-group deny-list entry. Presence of the key marks `account` as
/// currently denied for `group_id` — the receive-side network filter drops
/// state deltas they sign before the cross-DAG check runs. Cleared on
/// `MemberAdded` for the same `(group_id, account)` pair so re-adding a
/// previously-removed member transparently re-allows their traffic.
///
/// Denial is the negative of membership, so it is keyed the same way: by
/// [`AccountId`], which silences every device the denied person holds rather
/// than only the one whose key happened to author the removal.
///
/// Key layout: `prefix(1) + group_id(32) + account(32)` = 65 bytes —
/// same shape as `GroupMember` so prefix scans over `(group_id, *)` work
/// the same way.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupDeniedMember(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupDeniedMember {
    #[must_use]
    pub fn new(group_id: [u8; 32], account: AccountId) -> Self {
        Self(Key(GenericArray::from([GROUP_DENIED_MEMBER_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*account.as_bytes()))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn account(&self) -> AccountId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        AccountId::from(id)
    }
}

impl AsKeyParts for GroupDeniedMember {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupDeniedMember {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupDeniedMember {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupDeniedMember")
            .field("group_id", &self.group_id())
            .field("account", &self.account())
            .finish()
    }
}

/// Device→account binding for a group (see [`GROUP_DEVICE_BINDING_PREFIX`]).
///
/// One row per enrolled device. Key layout `prefix(1) + group_id(32) +
/// device_id(32)` = 65 bytes, the same shape as [`GroupMember`], so prefix
/// scans over `(group_id, *)` enumerate a group's devices exactly the way they
/// enumerate its members.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupDeviceBinding(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupDeviceBinding {
    #[must_use]
    pub fn new(group_id: [u8; 32], device_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([GROUP_DEVICE_BINDING_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(device_id))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn device_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        id
    }
}

impl AsKeyParts for GroupDeviceBinding {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupDeviceBinding {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupDeviceBinding {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupDeviceBinding")
            .field("group_id", &self.group_id())
            .field("device_id", &self.device_id())
            .finish()
    }
}

/// The binding a [`GroupDeviceBinding`] row carries.
///
/// `key_epoch` is retained deliberately. Whether the account has since rotated
/// past the root key that signed this device's certificate cannot be decided
/// when the link applies — at that moment only the rotations seen so far are
/// known, so the answer would depend on delivery order. Storing the signing
/// epoch lets the read side drop superseded bindings once the account's current
/// epoch is known, which makes the result a function of the op *set* rather
/// than its arrival order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupDeviceBindingValue {
    /// The account this device speaks for.
    pub account: [u8; 32],
    /// Ed25519 key whose signature counts as this device's.
    pub sign_pk: [u8; 32],
    /// X25519 key wrapped scope keys are delivered to.
    pub kem_pk: [u8; 32],
    /// Device key-rotation epoch; a link must strictly exceed it to supersede.
    pub device_epoch: u32,
    /// Account root-key epoch that signed this device's certificate.
    pub key_epoch: u32,
}

/// Revocation tombstone for a device (see [`GROUP_REVOKED_DEVICE_PREFIX`]).
///
/// A separate row family from [`GroupDeviceBinding`], not a flag on it. That is
/// what makes revocation order-independent: a revocation that applies *before*
/// the link it withdraws still wins, because every link consults this family
/// first. As a flag on the binding, a revoke-then-link arrival order would
/// silently resurrect the device.
///
/// Terminal — re-enrolling a machine mints a fresh device id — so a replica id
/// is never reused and the CRDT planes keep one writer per replica across a
/// revoke/re-add cycle.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupRevokedDevice(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupRevokedDevice {
    #[must_use]
    pub fn new(group_id: [u8; 32], device_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([GROUP_REVOKED_DEVICE_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(device_id))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn device_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        id
    }
}

impl AsKeyParts for GroupRevokedDevice {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupRevokedDevice {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupRevokedDevice {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupRevokedDevice")
            .field("group_id", &self.group_id())
            .field("device_id", &self.device_id())
            .finish()
    }
}

/// An account's current root key within a group (see [`GROUP_ACCOUNT_KEY_PREFIX`]).
///
/// Written the first time the group sees any credential for the account, and
/// advanced by each rotation. Key layout `prefix(1) + group_id(32) +
/// account_id(32)`.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupAccountKey(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupAccountKey {
    #[must_use]
    pub fn new(group_id: [u8; 32], account_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([GROUP_ACCOUNT_KEY_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(account_id))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn account_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        id
    }
}

impl AsKeyParts for GroupAccountKey {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupAccountKey {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupAccountKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupAccountKey")
            .field("group_id", &self.group_id())
            .field("account_id", &self.account_id())
            .finish()
    }
}

/// The root key an account currently holds in a group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupAccountKeyValue {
    /// Highest root-key epoch this group has established for the account.
    pub epoch: u32,
    /// The root key at `epoch` — the only key whose device certificates this
    /// group still accepts.
    ///
    /// Deliberately the *current* key and not the genesis one. This row exists to
    /// answer "may this certificate still be honoured", which a rotation is
    /// supposed to change; the account's tie to a member is a separate question,
    /// answered by [`GroupAccountEndorser`] rather than by any key here. The row
    /// used to carry the genesis key alongside for that purpose, back when an
    /// account was rooted at its owner's namespace identity — once the root became
    /// a dedicated offline key, which is a member nowhere, nothing could read it
    /// and it stopped being carried.
    pub root_pk: [u8; 32],
}

/// A member key that vouched for an account in a group. Key layout
/// `prefix(1) + group_id(32) + account_id(32) + member_pk(32)` = 97 bytes.
///
/// **The account's tie to a member, and the reason it is a row rather than a
/// field.** The account row's `genesis_root_pk` used to be that tie, back when
/// an account was rooted at its owner's namespace identity. The root is now a
/// dedicated offline key which is a member *nowhere*, so the tie moved to the
/// endorsement carried on each link — and an endorsement is per-op, so it has
/// to be persisted or the group forgets who vouched the moment the op is
/// applied.
///
/// A grow-only **set**, never a single field. Two links for one account may
/// legitimately carry different endorsers, so storing "the" endorser would make
/// the stored value depend on which link folded last — order-dependent state on
/// the authorization path, which is the failure this plane has hit repeatedly.
/// Set union is a join, so every replica converges on the same endorser set and
/// the question asked of it — "is *any* endorser a member at this cut" — is
/// order-independent.
///
/// Valueless: the key carries everything. There is nothing to record about an
/// endorsement beyond that it happened and was verified before the row was
/// written.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupAccountEndorser(
    Key<(
        GroupPrefix,
        GroupIdComponent,
        GroupIdComponent,
        GroupIdComponent,
    )>,
);

impl GroupAccountEndorser {
    #[must_use]
    pub fn new(group_id: [u8; 32], account_id: [u8; 32], member: AccountId) -> Self {
        Self(Key(GenericArray::from([GROUP_ACCOUNT_ENDORSER_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(account_id))
            .concat(GenericArray::from(*member.as_bytes()))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn account_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[33..65]);
        id
    }

    #[must_use]
    pub fn member(&self) -> AccountId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[65..]);
        AccountId::from(id)
    }
}

impl AsKeyParts for GroupAccountEndorser {
    type Components = (
        GroupPrefix,
        GroupIdComponent,
        GroupIdComponent,
        GroupIdComponent,
    );

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupAccountEndorser {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

/// This node's device — a **singleton**, keyed by nothing but its own prefix
/// (see [`NODE_DEVICE_IDENTITY_PREFIX`]).
///
/// Node-level rather than per-namespace, because a device is one installation.
/// A row per namespace made one laptop into five devices, each with its own
/// replica id and agreement key, for a distinction nothing downstream wanted:
/// the certificate binds one device to one signing key, and that key is
/// node-level too.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NodeDeviceIdentity(Key<(GroupPrefix,)>);

impl NodeDeviceIdentity {
    #[must_use]
    pub fn new() -> Self {
        Self(Key(GenericArray::from([NODE_DEVICE_IDENTITY_PREFIX])))
    }
}

impl Default for NodeDeviceIdentity {
    fn default() -> Self {
        Self::new()
    }
}

impl AsKeyParts for NodeDeviceIdentity {
    type Components = (GroupPrefix,);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for NodeDeviceIdentity {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for NodeDeviceIdentity {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NodeDeviceIdentity").finish()
    }
}

/// This node's account root secret — a **singleton**, keyed by nothing but its
/// own prefix (see [`NODE_ACCOUNT_ROOT_PREFIX`]).
///
/// Node-level, which is the whole point: it is the one key that survives losing
/// every device, so it is what certifies a replacement. It is also what the
/// account id is derived from, so one root means one account wherever it speaks.
///
/// A one-byte key rather than a sentinel id, so the singleton-ness is in the type
/// and there is no "which id means the real one" question for a later reader.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NodeAccountRoot(Key<(GroupPrefix,)>);

impl NodeAccountRoot {
    #[must_use]
    pub fn new() -> Self {
        Self(Key(GenericArray::from([NODE_ACCOUNT_ROOT_PREFIX])))
    }
}

impl Default for NodeAccountRoot {
    fn default() -> Self {
        Self::new()
    }
}

impl AsKeyParts for NodeAccountRoot {
    type Components = (GroupPrefix,);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for NodeAccountRoot {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for NodeAccountRoot {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("NodeAccountRoot")
    }
}

/// The secret a [`NodeAccountRoot`] row carries.
///
/// The **only** thing that can certify a device for any of this node's accounts,
/// and therefore the only thing that can recover one after every device is lost.
/// Intended to be backed up out of band — paper or hardware — because losing it
/// alongside the devices means no recovery at all.
#[derive(Clone, Eq, PartialEq, ZeroizeOnDrop)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NodeAccountRootValue {
    /// Ed25519 secret that roots every account this node owns.
    pub root_secret: [u8; 32],
}

/// Redacted by hand, never derived — same discipline as the other secret-bearing
/// values here. This one is the most sensitive of them: it is the recovery key.
impl Debug for NodeAccountRootValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeAccountRootValue")
            .field("root_secret", &"[redacted]")
            .finish()
    }
}

/// The device identity a [`NodeDeviceIdentity`] row carries.
///
/// `kem_secret` is the private half of the X25519 key published in this
/// device's certificate, and it is the **only** thing that can unwrap a scope
/// key addressed to this device. It is stored, not derived from the namespace
/// signing key, because the whole point of a device KEM key is that it is
/// revocable independently of the identity that certified it — deriving it
/// would tie the two lifetimes back together.
///
/// Deliberately **not** `Copy`, and zeroized on drop. `Copy` would let the secret
/// be duplicated implicitly — every read, every move producing another copy the
/// wipe never reaches — which is the same reason
/// [`calimero_crypto::X25519SecretKey`] is not `Copy` either. A `Drop` impl and
/// `Copy` are mutually exclusive, so dropping `Copy` is what makes the wipe
/// possible at all.
#[derive(Clone, Eq, PartialEq, ZeroizeOnDrop)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct NodeDeviceIdentityValue {
    /// Epoch-0 root key of the account this node's device belongs to.
    ///
    /// Stored rather than assumed to be this node's own root: a paired device
    /// adopts an account rooted at ANOTHER node's key, so the row has to name
    /// whose account it is — the reader cannot derive it.
    pub account_root_pk: [u8; 32],
    /// The `DeviceId` this node speaks as. One row per node, so this is the id
    /// it speaks as in every namespace it takes part in.
    pub device_id: [u8; 32],
    /// X25519 secret matching the certificate's `kem_pk`.
    pub kem_secret: [u8; 32],
}

/// Redacted by hand, never derived. `kem_secret` is the only thing that can
/// unwrap a scope key addressed to this device, and a derived `Debug` prints it
/// — one `tracing` field, one `dbg!`, one error context and the secret is in a
/// log. Mirrors the same discipline `calimero_crypto::SharedKey` applies.
impl Debug for NodeDeviceIdentityValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeDeviceIdentityValue")
            .field("device_id", &self.device_id)
            .field("kem_secret", &"[redacted]")
            .finish()
    }
}

/// Namespace-root inherited-deny entry (see [`GROUP_INHERITED_DENIED_MEMBER_PREFIX`]).
/// Presence marks `account`, keyed to the namespace root, as inherited-denied:
/// the receive filter drops their deltas to any descendant subgroup they reached
/// only by inheritance. Same 65-byte layout as `GroupDeniedMember`
/// (`prefix(1) + group_id(32) + account(32)`) so `(group_id, *)` prefix scans work
/// identically, but a distinct column so the two deny views never collide.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupInheritedDeniedMember(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupInheritedDeniedMember {
    #[must_use]
    pub fn new(group_id: [u8; 32], account: AccountId) -> Self {
        Self(Key(GenericArray::from([
            GROUP_INHERITED_DENIED_MEMBER_PREFIX,
        ])
        .concat(GenericArray::from(group_id))
        .concat(GenericArray::from(*account.as_bytes()))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn account(&self) -> AccountId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        AccountId::from(id)
    }
}

impl AsKeyParts for GroupInheritedDeniedMember {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupInheritedDeniedMember {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupInheritedDeniedMember {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupInheritedDeniedMember")
            .field("group_id", &self.group_id())
            .field("account", &self.account())
            .finish()
    }
}

/// How an identity left a group. Recorded on the re-entry block, because what
/// can readmit them depends on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub enum GroupExitReason {
    /// An admin removed them (`MemberRemoved`). Only an admin `MemberAdded`
    /// readmits them — no invitation, however freshly issued, will.
    Removed,
    /// They left of their own accord (`MemberLeft`). A fresh invitation whose
    /// nonce they have not already consumed readmits them, as does an admin
    /// `MemberAdded`.
    Left,
}

/// Value for [`GroupReentryBlock`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupReentryBlockValue {
    pub reason: GroupExitReason,
}

/// Blocks an account from re-entering a group after they exited it.
/// Key layout: `prefix(1) + group_id(32) + account(32)` = 65 bytes.
///
/// Account-keyed so someone who left cannot walk back in from a second
/// device — the block follows the person, not the key that signed the exit.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupReentryBlock(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupReentryBlock {
    #[must_use]
    pub fn new(group_id: [u8; 32], account: AccountId) -> Self {
        Self(Key(GenericArray::from([GROUP_REENTRY_BLOCK_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*account.as_bytes()))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn account(&self) -> AccountId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        AccountId::from(id)
    }
}

impl AsKeyParts for GroupReentryBlock {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupReentryBlock {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupReentryBlock {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupReentryBlock")
            .field("group_id", &self.group_id())
            .field("account", &self.account())
            .finish()
    }
}

/// Records that `account` has already used the invitation identified by
/// `invitation_nonce` to join `group_id`.
/// Key layout: `prefix(1) + group_id(32) + account(32) + nonce(32)` = 97 bytes.
///
/// Account-keyed so one invitation cannot be redeemed once per device.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupConsumedInvitation(
    Key<(
        GroupPrefix,
        GroupIdComponent,
        GroupIdComponent,
        GroupIdComponent,
    )>,
);

impl GroupConsumedInvitation {
    #[must_use]
    pub fn new(group_id: [u8; 32], account: AccountId, invitation_nonce: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([GROUP_CONSUMED_INVITATION_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*account.as_bytes()))
            .concat(GenericArray::from(invitation_nonce))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn account(&self) -> AccountId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[33..65]);
        AccountId::from(id)
    }

    #[must_use]
    pub fn invitation_nonce(&self) -> [u8; 32] {
        let mut nonce = [0; 32];
        nonce.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[65..]);
        nonce
    }
}

impl AsKeyParts for GroupConsumedInvitation {
    type Components = (
        GroupPrefix,
        GroupIdComponent,
        GroupIdComponent,
        GroupIdComponent,
    );

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupConsumedInvitation {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupConsumedInvitation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupConsumedInvitation")
            .field("group_id", &self.group_id())
            .field("account", &self.account())
            .field("invitation_nonce", &self.invitation_nonce())
            .finish()
    }
}

/// Pending-key-rotation worklist entry. Presence of the key means: `group_id` owes
/// a forward-secrecy rotation because `departed` left it, and none has landed yet.
///
/// The value is `()` — presence of the key IS the marker, like [`GroupDeniedMember`].
/// Keyed by `(group_id, departed)` rather than `group_id` alone so two members
/// leaving the same group concurrently each get their own row: one rotation may
/// discharge both, but neither row is silently lost if only one rotation lands.
///
/// Key layout: `prefix(1) + group_id(32) + departed(32)` = 65 bytes — the same shape
/// as [`GroupMember`] / [`GroupDeniedMember`], so a `(group_id, *)` prefix scan
/// enumerates every rotation a group owes, and a full-prefix scan enumerates the
/// node's whole rotation backlog (what the rotator drains at startup).
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupPendingKeyRotation(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupPendingKeyRotation {
    #[must_use]
    pub fn new(group_id: [u8; 32], departed: AccountId) -> Self {
        Self(Key(GenericArray::from([GROUP_PENDING_KEY_ROTATION_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*departed.as_bytes()))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn departed(&self) -> AccountId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        AccountId::from(id)
    }
}

impl AsKeyParts for GroupPendingKeyRotation {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupPendingKeyRotation {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupPendingKeyRotation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupPendingKeyRotation")
            .field("group_id", &self.group_id())
            .field("departed", &self.departed())
            .finish()
    }
}

pub const GROUP_PENDING_DEVICE_ROTATION_PREFIX: u8 = 0x49;

/// A rotation this group owes because a DEVICE was revoked without one.
///
/// The sibling of [`GroupPendingKeyRotation`], and deliberately a separate row
/// rather than a reuse of it, because the two discharge differently. A departure
/// rotates *excluding the departed account*; a device revocation rotates
/// **excluding nobody by name** — the revoked device is already gone from the
/// recipient list, since that list is built from live bindings, and the account
/// it belonged to keeps every other device it holds. Keying both by
/// `(group_id, 32 bytes)` and telling them apart by prefix is what stops a
/// discharge for one being mistaken for the other and cutting off a member who
/// never left.
///
/// The value is `()` — presence of the key IS the marker.
///
/// Key layout: `prefix(1) + group_id(32) + device(32)` = 65 bytes, matching
/// [`GroupPendingKeyRotation`], so a `(group_id, *)` prefix scan enumerates what
/// one group owes and a full-prefix scan drains the node's whole backlog at
/// startup.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupPendingDeviceRotation(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupPendingDeviceRotation {
    #[must_use]
    pub fn new(group_id: [u8; 32], device: DeviceId) -> Self {
        Self(Key(GenericArray::from([
            GROUP_PENDING_DEVICE_ROTATION_PREFIX,
        ])
        .concat(GenericArray::from(group_id))
        .concat(GenericArray::from(*device.as_bytes()))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn device(&self) -> DeviceId {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        DeviceId::from(id)
    }
}

impl AsKeyParts for GroupPendingDeviceRotation {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupPendingDeviceRotation {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupPendingDeviceRotation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupPendingDeviceRotation")
            .field("group_id", &self.group_id())
            .field("device", &self.device())
            .finish()
    }
}

/// Value for [`GroupKeyEntry`]. The raw 32-byte AES-256 group key plus its
/// ordering metadata.
///
/// `epoch` is the deterministic, DAG-derived sequence of the governance op that
/// introduced this key (or `0` for a genesis / bootstrap key). It — not the
/// wall-clock `created_at` — is what selects the "current" (latest) key for
/// encryption, so all nodes agree even under clock skew or two rotations within
/// the same second. `created_at` is retained for diagnostics only.
///
/// `insertion_seq` is a per-group, strictly increasing counter stamped when the
/// entry is first written, i.e. the order in which *this node* learned the key.
/// It exists solely to order keys that carry **no** DAG ordering — genesis and
/// direct-pull keys, which are all stamped `epoch = 0`. Without it a node
/// holding two epoch-`0` keys would tie-break by `key_id` (a sha256 hash) and
/// could resolve the *older* key as current. It is deliberately NOT consulted
/// for equal non-zero epochs: those are concurrent rotations, whose convergence
/// across nodes depends on the hash tie-break rather than on local arrival
/// order. See `GroupKeyring::load_current_key_record`.
///
/// # On-disk compatibility
///
/// `epoch` and `insertion_seq` were both appended to a struct that already had
/// rows on disk (`epoch` in #3114, `insertion_seq` here). Borsh has no field
/// tags, so a derived `BorshDeserialize` would reject every pre-existing row as
/// a truncated buffer, bricking the group keyring on any node that upgrades in
/// place. [`BorshDeserialize`] is therefore hand-written below to read the two
/// trailing `u64`s **optionally**: a buffer that ends after `created_at`
/// decodes as `epoch = 0, insertion_seq = 0`, and one that ends after `epoch`
/// decodes as `insertion_seq = 0`. Both defaults are the value the field would
/// have carried anyway — a key written before epochs existed is a genesis-era
/// key (`epoch 0`), and one written before insertion order was tracked has no
/// recorded arrival order (`0`). The row is rewritten with the full layout the
/// next time `store_key_with_epoch` raises its epoch.
///
/// Serialization is still derived, so every *new* write is the full four-field
/// layout; only the read side is lenient. Any field added after this one must
/// extend the same tail-optional pattern rather than re-deriving.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize))]
pub struct GroupKeyValue {
    pub group_key: [u8; 32],
    pub created_at: u64,
    pub epoch: u64,
    pub insertion_seq: u64,
}

/// Read a `u64` that may legitimately be absent because the buffer predates the
/// field, distinguishing "nothing left to read" (an older row — return `None`)
/// from "a partial value" (genuine corruption — an error).
#[cfg(feature = "borsh")]
fn read_optional_trailing_u64<R: borsh::io::Read>(
    reader: &mut R,
) -> borsh::io::Result<Option<u64>> {
    let mut buf = [0_u8; 8];
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == borsh::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    match filled {
        0 => Ok(None),
        8 => Ok(Some(u64::from_le_bytes(buf))),
        _ => Err(borsh::io::Error::new(
            borsh::io::ErrorKind::InvalidData,
            "GroupKeyValue: truncated trailing u64",
        )),
    }
}

#[cfg(feature = "borsh")]
impl BorshDeserialize for GroupKeyValue {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        // The two leading fields have always been present — a buffer missing
        // either of them is corrupt, not old, so these stay strict.
        let group_key = <[u8; 32]>::deserialize_reader(reader)?;
        let created_at = u64::deserialize_reader(reader)?;
        let epoch = read_optional_trailing_u64(reader)?.unwrap_or(0);
        let insertion_seq = read_optional_trailing_u64(reader)?.unwrap_or(0);
        Ok(Self {
            group_key,
            created_at,
            epoch,
            insertion_seq,
        })
    }
}

/// Durable pending-self-purge marker, keyed by `namespace_id` (the root
/// group's ContextGroupId).
///
/// Key layout: `PENDING_SELF_PURGE_PREFIX (1 byte) + namespace_id (32 bytes)`
/// = 33 bytes — the same shape as [`NamespaceParticipation`]. A `(prefix,
/// namespace_id)` range scan over this column family enumerates every marked
/// namespace in `namespace_id` order. The value is `()` — presence of the
/// key IS the marker (like [`GroupDeniedMember`] / [`GroupChildIndex`]).
///
/// Presence means: this node was confirmed TEE-self-evicted from the
/// namespace and its local-state cascade purge has not yet fully completed.
/// Written before the cascade runs (so a crash mid-cascade is covered) and
/// cleared only once the row purge fully succeeds. The startup
/// reconcile sweep enumerates these markers and completes ONLY the
/// namespaces still flagged AND still-evicted — see the `self_purge` module
/// in `calimero-context` (#2721).
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct PendingSelfPurge(Key<(GroupPrefix, GroupIdComponent)>);

impl PendingSelfPurge {
    #[must_use]
    pub fn new(namespace_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([PENDING_SELF_PURGE_PREFIX])
            .concat(GenericArray::from(namespace_id))))
    }

    #[must_use]
    pub fn namespace_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 33]>::as_ref(&self.0)[1..]);
        id
    }
}

impl AsKeyParts for PendingSelfPurge {
    type Components = (GroupPrefix, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for PendingSelfPurge {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for PendingSelfPurge {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingSelfPurge")
            .field("namespace_id", &self.namespace_id())
            .finish()
    }
}

#[cfg(test)]
mod tests {

    /// The 65-byte families in `Column::Group` are byte-indistinguishable by
    /// layout alone, so every scan must stop on its own prefix.
    ///
    /// Three walks in this repo learned that the hard way — the group keyring,
    /// the migration cohort, and the namespace gov-op log all read a neighbour's
    /// rows because "same width, same id in the same place" looked like a match.
    /// On a namespace ROOT the ids really are equal, so the id check cannot
    /// separate them and the predicate is the only thing that can.
    #[test]
    fn same_width_group_families_are_only_separable_by_prefix() {
        let id = [0x77u8; 32];
        let gov = NamespaceGovOp::new(id, [0x01; 32]);
        let binding = GroupDeviceBinding::new(id, [0x01; 32]);

        // Same width, and the id lands in the same bytes...
        assert_eq!(
            gov.as_key().as_bytes().len(),
            binding.as_key().as_bytes().len(),
            "these two families are the same width, which is why a walk confuses them"
        );
        assert_eq!(
            &gov.as_key().as_bytes()[1..33],
            &binding.as_key().as_bytes()[1..33],
            "...and carry the same id in the same position, so an id check cannot \
             tell them apart"
        );

        // ...so only the family byte separates them, and the predicate reads it.
        assert!(gov.is_gov_op_row());
        let binding_as_gov = NamespaceGovOp::try_from_parts(Key(*GenericArray::from_slice(
            binding.as_key().as_bytes(),
        )))
        .expect("a binding row parses as a gov-op key: identical layout");
        assert!(
            !binding_as_gov.is_gov_op_row(),
            "a walk bounded on this predicate stops before reading a binding's \
             value as op bytes"
        );
    }
    use super::*;

    #[test]
    fn group_meta_roundtrip() {
        let id = [0xAB; 32];
        let key = GroupMeta::new(id);
        assert_eq!(key.group_id(), id);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_META_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 33);
    }

    #[test]
    fn group_member_roundtrip() {
        let gid = [0xCD; 32];
        let account = AccountId::from([0xEF; 32]);
        let key = GroupMember::new(gid, account);
        assert_eq!(key.group_id(), gid);
        assert_eq!(key.account(), account);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_MEMBER_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 65);
    }

    #[test]
    fn group_context_member_cap_roundtrip() {
        // 97-byte 4-component key with a field order (group, context, member)
        // distinct from `GroupMemberContext`'s (group, member, context).
        // Distinct byte patterns per component prove each accessor reads its
        // own [1..33]/[33..65]/[65..97] slice, so the two shapes can't alias.
        let gid = [0x10; 32];
        let context_id = PrimitiveContextId::from([0x20; 32]);
        let member = AccountId::from([0x30; 32]);
        let key = GroupContextMemberCap::new(gid, context_id, member);

        assert_eq!(key.group_id(), gid);
        assert_eq!(key.context_id(), context_id);
        assert_eq!(key.member(), member);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_CONTEXT_MEMBER_CAP_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 97);

        // Full decode round-trip through the exact-length slice.
        let bytes = key.as_key().as_bytes();
        let parts = Key::<<GroupContextMemberCap as AsKeyParts>::Components>::try_from_slice(bytes)
            .expect("exact-length slice must decode");
        let restored = GroupContextMemberCap::try_from_parts(parts).unwrap();
        assert_eq!(restored.as_key().as_bytes(), bytes);

        // A mis-sized slice must be rejected (no length framing on the key).
        assert!(
            Key::<<GroupContextMemberCap as AsKeyParts>::Components>::try_from_slice(&bytes[..96])
                .is_none()
        );
    }

    #[test]
    fn pending_self_purge_roundtrip() {
        let ns = [0x77; 32];
        let key = PendingSelfPurge::new(ns);
        assert_eq!(key.namespace_id(), ns);
        assert_eq!(key.as_key().as_bytes()[0], PENDING_SELF_PURGE_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 33);
    }

    /// `AutoFollowFlags::default()` is the contract `add_group_member`'s
    /// `.unwrap_or_default()` fallback relies on. Pin the exact values
    /// here so a future Default impl change is caught at compile-test
    /// time, not at runtime in production.
    #[test]
    fn auto_follow_flags_default_is_contexts_true_subgroups_false() {
        assert_eq!(
            AutoFollowFlags::default(),
            AutoFollowFlags {
                contexts: true,
                subgroups: false,
            }
        );
    }

    /// A record with a new-format (four-field) layout must round-trip.
    #[cfg(feature = "borsh")]
    #[test]
    fn group_member_value_roundtrip_with_flags() {
        let value = GroupMemberValue {
            role: GroupMemberRole::Member,
            private_key: None,
            sender_key: Some([0x22; 32]),
            auto_follow: AutoFollowFlags {
                contexts: true,
                subgroups: false,
            },
        };
        let bytes = borsh::to_vec(&value).unwrap();
        let decoded: GroupMemberValue = borsh::from_slice(&bytes).unwrap();
        assert_eq!(decoded.role, value.role);
        assert_eq!(decoded.private_key, value.private_key);
        assert_eq!(decoded.sender_key, value.sender_key);
        assert_eq!(decoded.auto_follow, value.auto_follow);
    }

    /// Corruption guard: a record with one trailing byte (partial
    /// `auto_follow` field) must fail to deserialize rather than
    /// silently defaulting. This catches the class of bug where data
    /// truncation or partial writes would otherwise be invisible.
    #[cfg(feature = "borsh")]
    #[test]
    fn group_member_value_rejects_partial_auto_follow() {
        use borsh::BorshSerialize;

        #[derive(BorshSerialize)]
        struct PartialGroupMemberValue {
            role: GroupMemberRole,
            private_key: Option<[u8; 32]>,
            sender_key: Option<[u8; 32]>,
            truncated: bool,
        }

        let partial = PartialGroupMemberValue {
            role: GroupMemberRole::Member,
            private_key: None,
            sender_key: None,
            truncated: true,
        };
        let bytes = borsh::to_vec(&partial).unwrap();
        let err = borsh::from_slice::<GroupMemberValue>(&bytes).unwrap_err();
        assert_eq!(
            err.kind(),
            borsh::io::ErrorKind::InvalidData,
            "a truncated record must fail loudly, not default its trailing field"
        );
    }

    #[test]
    fn group_context_index_roundtrip() {
        let gid = [0x11; 32];
        let cid = PrimitiveContextId::from([0x22; 32]);
        let key = GroupContextIndex::new(gid, cid);
        assert_eq!(key.group_id(), gid);
        assert_eq!(key.context_id(), cid);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_CONTEXT_INDEX_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 65);
    }

    #[test]
    fn context_group_ref_roundtrip() {
        let cid = PrimitiveContextId::from([0x33; 32]);
        let key = ContextGroupRef::new(cid);
        assert_eq!(key.context_id(), cid);
        assert_eq!(key.as_key().as_bytes()[0], CONTEXT_GROUP_REF_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 33);
    }

    #[test]
    fn group_upgrade_key_roundtrip() {
        let gid = [0x44; 32];
        let key = GroupUpgradeKey::new(gid);
        assert_eq!(key.group_id(), gid);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_UPGRADE_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 33);
    }

    #[test]
    fn group_fleet_completion_key_roundtrip() {
        let gid = [0x44; 32];
        let key = GroupFleetCompletion::new(gid);
        assert_eq!(key.group_id(), gid);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_FLEET_COMPLETION_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 33);
        // Same width and the same id bytes as the upgrade row it stamps, so the
        // prefix is the only thing keeping a prefix-bounded scan off it.
        assert_ne!(GROUP_FLEET_COMPLETION_PREFIX, GROUP_UPGRADE_PREFIX);
    }

    #[test]
    fn group_upgrade_ladder_key_roundtrip() {
        let gid = [0x47; 32];
        let key = GroupUpgradeLadder::new(gid);
        assert_eq!(key.group_id(), gid);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_UPGRADE_LADDER_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 33);
    }

    /// Every prefix in this column, not a subset: the families are keyed only by
    /// this byte and several are byte-identical in length, so a partial list
    /// leaves real collisions uncaught. Re-derive with `grep 'u8 = 0x'` on this
    /// file when adding one.
    #[test]
    fn distinct_prefixes() {
        let prefixes = [
            ("GROUP_META", GROUP_META_PREFIX),
            ("GROUP_MEMBER", GROUP_MEMBER_PREFIX),
            ("GROUP_CONTEXT_INDEX", GROUP_CONTEXT_INDEX_PREFIX),
            ("CONTEXT_GROUP_REF", CONTEXT_GROUP_REF_PREFIX),
            ("GROUP_UPGRADE", GROUP_UPGRADE_PREFIX),
            ("GROUP_MEMBER_CAPABILITY", GROUP_MEMBER_CAPABILITY_PREFIX),
            ("GROUP_REENTRY_BLOCK", GROUP_REENTRY_BLOCK_PREFIX),
            (
                "GROUP_CONSUMED_INVITATION",
                GROUP_CONSUMED_INVITATION_PREFIX,
            ),
            ("GROUP_DEFAULT_CAPS", GROUP_DEFAULT_CAPS_PREFIX),
            ("GROUP_SUBGROUP_VIS", GROUP_SUBGROUP_VIS_PREFIX),
            ("GROUP_MEMBER_METADATA", GROUP_MEMBER_METADATA_PREFIX),
            ("GROUP_METADATA", GROUP_METADATA_PREFIX),
            ("GROUP_CONTEXT_METADATA", GROUP_CONTEXT_METADATA_PREFIX),
            ("GROUP_OP_LOG", GROUP_OP_LOG_PREFIX),
            ("GROUP_OP_HEAD", GROUP_OP_HEAD_PREFIX),
            ("GROUP_MEMBER_CONTEXT", GROUP_MEMBER_CONTEXT_PREFIX),
            ("GROUP_CONTEXT_MEMBER_CAP", GROUP_CONTEXT_MEMBER_CAP_PREFIX),
            ("GROUP_PARENT_REF", GROUP_PARENT_REF_PREFIX),
            ("GROUP_CHILD_INDEX", GROUP_CHILD_INDEX_PREFIX),
            ("NAMESPACE_PARTICIPATION", NAMESPACE_PARTICIPATION_PREFIX),
            ("CONTEXT_SERVICE_NAME", CONTEXT_SERVICE_NAME_PREFIX),
            ("NAMESPACE_GOV_OP", NAMESPACE_GOV_OP_PREFIX),
            ("NAMESPACE_GOV_HEAD", NAMESPACE_GOV_HEAD_PREFIX),
            ("GROUP_KEY", GROUP_KEY_PREFIX),
            ("GROUP_DENIED_MEMBER", GROUP_DENIED_MEMBER_PREFIX),
            (
                "GROUP_LOCAL_GOV_NONCE_WINDOW",
                GROUP_LOCAL_GOV_NONCE_WINDOW_PREFIX,
            ),
            ("PENDING_SELF_PURGE", PENDING_SELF_PURGE_PREFIX),
            ("GROUP_UPGRADE_LADDER", GROUP_UPGRADE_LADDER_PREFIX),
            (
                "GROUP_PENDING_KEY_ROTATION",
                GROUP_PENDING_KEY_ROTATION_PREFIX,
            ),
            (
                "GROUP_INHERITED_DENIED_MEMBER",
                GROUP_INHERITED_DENIED_MEMBER_PREFIX,
            ),
            ("GROUP_DEVICE_BINDING", GROUP_DEVICE_BINDING_PREFIX),
            ("GROUP_REVOKED_DEVICE", GROUP_REVOKED_DEVICE_PREFIX),
            ("GROUP_ACCOUNT_KEY", GROUP_ACCOUNT_KEY_PREFIX),
            ("NODE_DEVICE_IDENTITY", NODE_DEVICE_IDENTITY_PREFIX),
            ("NODE_ACCOUNT_ROOT", NODE_ACCOUNT_ROOT_PREFIX),
            ("GROUP_ACCOUNT_ENDORSER", GROUP_ACCOUNT_ENDORSER_PREFIX),
            (
                "NAMESPACE_BOOTSTRAP_INVITER",
                NAMESPACE_BOOTSTRAP_INVITER_PREFIX,
            ),
            ("GROUP_FLEET_COMPLETION", GROUP_FLEET_COMPLETION_PREFIX),
            ("NODE_IDENTITY", NODE_IDENTITY_PREFIX),
            (
                "GROUP_PENDING_DEVICE_ROTATION",
                GROUP_PENDING_DEVICE_ROTATION_PREFIX,
            ),
        ];
        for i in 0..prefixes.len() {
            for j in (i + 1)..prefixes.len() {
                let ((a, x), (b, y)) = (prefixes[i], prefixes[j]);
                assert_ne!(x, y, "{a} and {b} share prefix {x:#04X}");
            }
        }
    }

    #[test]
    fn group_member_metadata_roundtrip() {
        let gid = [0xDA; 32];
        let account = AccountId::from([0xDB; 32]);
        let key = GroupMemberMetadata::new(gid, account);
        assert_eq!(key.group_id(), gid);
        assert_eq!(key.member(), account);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_MEMBER_METADATA_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 65);
    }

    #[test]
    fn group_metadata_roundtrip() {
        let gid = [0xDC; 32];
        let key = GroupMetadata::new(gid);
        assert_eq!(key.group_id(), gid);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_METADATA_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 33);
    }

    #[test]
    fn group_context_metadata_roundtrip() {
        let gid = [0xDD; 32];
        let ctx = PrimitiveContextId::from([0xDE; 32]);
        let key = GroupContextMetadata::new(gid, ctx);
        assert_eq!(key.group_id(), gid);
        assert_eq!(key.context_id(), ctx);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_CONTEXT_METADATA_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 65);
    }

    #[test]
    fn group_op_log_roundtrip() {
        let gid = [0xE1; 32];
        let seq = 42u64;
        let key = GroupOpLog::new(gid, seq);
        assert_eq!(key.group_id(), gid);
        assert_eq!(key.sequence(), seq);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_OP_LOG_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 41);
    }

    #[test]
    fn group_op_log_ordering() {
        let gid = [0xE1; 32];
        let k1 = GroupOpLog::new(gid, 1);
        let k2 = GroupOpLog::new(gid, 2);
        let k100 = GroupOpLog::new(gid, 100);
        assert!(k1 < k2);
        assert!(k2 < k100);
    }

    #[test]
    fn group_op_head_roundtrip() {
        let gid = [0xF1; 32];
        let key = GroupOpHead::new(gid);
        assert_eq!(key.group_id(), gid);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_OP_HEAD_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 33);
    }

    #[cfg(feature = "borsh")]
    mod value_roundtrips {
        use borsh::{from_slice, to_vec};
        use calimero_account::AccountId;
        use calimero_primitives::application::ApplicationId;
        use calimero_primitives::context::GroupMemberRole;
        use calimero_primitives::identity::PublicKey as PrimitivePublicKey;

        use super::super::{
            AutoFollowFlags, GroupMemberValue, GroupMetaValue, GroupUpgradeStatus,
            GroupUpgradeValue,
        };

        #[test]
        fn group_meta_value_roundtrip() {
            let value = GroupMetaValue {
                bytecode_id: [0xAA; 32],
                target_application_id: ApplicationId::from([0xBB; 32]),
                created_at: 1_700_000_000,
                admin_identity: AccountId::from([0xCC; 32]),
                owner_identity: AccountId::from([0xCC; 32]),
                migration: None,
                auto_join: true,
            };

            let bytes = to_vec(&value).expect("serialize");
            let decoded: GroupMetaValue = from_slice(&bytes).expect("deserialize");

            assert_eq!(decoded.bytecode_id, value.bytecode_id);
            assert_eq!(decoded.target_application_id, value.target_application_id);
            assert_eq!(decoded.created_at, value.created_at);
            assert_eq!(decoded.admin_identity, value.admin_identity);
        }

        #[test]
        // `upgrade_policy` was dropped with no store-version gate, so a record
        // written before the removal must fail loudly, never shift into garbage.
        fn group_meta_value_with_legacy_policy_tag_is_rejected() {
            let value = GroupMetaValue {
                bytecode_id: [0x11; 32],
                target_application_id: ApplicationId::from([0x22; 32]),
                created_at: 1_700_000_000,
                admin_identity: AccountId::from([0x33; 32]),
                owner_identity: AccountId::from([0x33; 32]),
                migration: None,
                auto_join: true,
            };

            // Re-create the old layout: the policy tag sat between
            // `target_application_id` and `created_at`.
            let mut bytes = to_vec(&value).expect("serialize");
            let tag_offset = to_vec(&value.bytecode_id).expect("serialize").len()
                + to_vec(&value.target_application_id)
                    .expect("serialize")
                    .len();
            bytes.insert(tag_offset, 0);

            assert!(
                from_slice::<GroupMetaValue>(&bytes).is_err(),
                "a stored GroupMetaValue still carrying an upgrade-policy tag must be rejected"
            );
        }

        #[test]
        fn group_member_role_roundtrip() {
            for role in [GroupMemberRole::Admin, GroupMemberRole::Member] {
                let bytes = to_vec(&role).expect("serialize");
                let decoded: GroupMemberRole = from_slice(&bytes).expect("deserialize");
                assert_eq!(decoded, role);
            }
        }

        #[test]
        fn group_upgrade_value_in_progress_roundtrip() {
            let value = GroupUpgradeValue {
                from_version: "1.0.0".to_owned(),
                to_version: "2.0.0".to_owned(),
                migration: Some(vec![0xDE, 0xAD]),
                initiated_at: 1_700_000_000,
                initiated_by: PrimitivePublicKey::from([0x03; 32]),
                status: GroupUpgradeStatus::InProgress {
                    total: 10,
                    completed: 3,
                    failed: 1,
                },
                cascade_hlc: None,
                cascade_seq: None,
                to_state_version: 2,
            };

            let bytes = to_vec(&value).expect("serialize");
            let decoded: GroupUpgradeValue = from_slice(&bytes).expect("deserialize");

            assert_eq!(decoded.from_version, "1.0.0");
            assert_eq!(decoded.to_version, "2.0.0");
            assert_eq!(decoded.to_state_version, 2);
            assert_eq!(decoded.migration, Some(vec![0xDE, 0xAD]));
            assert_eq!(decoded.initiated_at, value.initiated_at);
            assert_eq!(decoded.initiated_by, value.initiated_by);
            match decoded.status {
                GroupUpgradeStatus::InProgress {
                    total,
                    completed,
                    failed,
                } => {
                    assert_eq!(total, 10);
                    assert_eq!(completed, 3);
                    assert_eq!(failed, 1);
                }
                other => panic!("expected InProgress, got {other:?}"),
            }
            assert_eq!(decoded.cascade_hlc, None);
        }

        #[test]
        fn group_member_value_roundtrip() {
            for role in [GroupMemberRole::Admin, GroupMemberRole::Member] {
                let value = GroupMemberValue {
                    role: role.clone(),
                    private_key: Some([0xAA; 32]),
                    sender_key: Some([0xBB; 32]),
                    auto_follow: AutoFollowFlags::default(),
                };
                let bytes = to_vec(&value).expect("serialize");
                let decoded: GroupMemberValue = from_slice(&bytes).expect("deserialize");
                assert_eq!(decoded.role, role);
                assert_eq!(decoded.private_key, Some([0xAA; 32]));
                assert_eq!(decoded.sender_key, Some([0xBB; 32]));
                assert_eq!(decoded.auto_follow, AutoFollowFlags::default());
            }
        }

        #[test]
        fn group_member_value_without_keys_roundtrip() {
            let value = GroupMemberValue {
                role: GroupMemberRole::Member,
                private_key: None,
                sender_key: None,
                auto_follow: AutoFollowFlags::default(),
            };
            let bytes = to_vec(&value).expect("serialize");
            let decoded: GroupMemberValue = from_slice(&bytes).expect("deserialize");
            assert_eq!(decoded.role, GroupMemberRole::Member);
            assert_eq!(decoded.private_key, None);
            assert_eq!(decoded.sender_key, None);
            assert_eq!(decoded.auto_follow, AutoFollowFlags::default());
        }

        #[test]
        fn group_upgrade_value_no_migration_roundtrip() {
            let value = GroupUpgradeValue {
                from_version: "3.0.0".to_owned(),
                to_version: "4.0.0".to_owned(),
                migration: None,
                initiated_at: 1_700_000_000,
                initiated_by: PrimitivePublicKey::from([0x06; 32]),
                status: GroupUpgradeStatus::Completed {
                    completed_at: Some(1_700_001_000),
                },
                cascade_hlc: None,
                cascade_seq: None,
                to_state_version: 4,
            };

            let bytes = to_vec(&value).expect("serialize");
            let decoded: GroupUpgradeValue = from_slice(&bytes).expect("deserialize");

            assert_eq!(decoded.from_version, "3.0.0");
            assert_eq!(decoded.to_version, "4.0.0");
            assert_eq!(decoded.to_state_version, 4);
            assert_eq!(decoded.migration, None);
            match decoded.status {
                GroupUpgradeStatus::Completed { completed_at } => {
                    assert_eq!(completed_at, Some(1_700_001_000));
                }
                other => panic!("expected Completed, got {other:?}"),
            }
            assert_eq!(decoded.cascade_hlc, None);
        }
    }
}

#[cfg(all(test, feature = "borsh"))]
mod cascade_hlc_borsh_tests {
    use borsh::{to_vec, BorshDeserialize, BorshSerialize};
    use calimero_primitives::identity::PublicKey as PrimitivePublicKey;
    use calimero_storage::logical_clock::HybridTimestamp;

    use super::{GroupUpgradeStatus, GroupUpgradeValue};

    fn sample(cascade_hlc: Option<HybridTimestamp>) -> GroupUpgradeValue {
        GroupUpgradeValue {
            from_version: "1.0.0".to_owned(),
            to_version: "2.0.0".to_owned(),
            migration: Some(vec![1, 2, 3]),
            initiated_at: 1_700_000_000,
            initiated_by: PrimitivePublicKey::from([7u8; 32]),
            status: GroupUpgradeStatus::Completed { completed_at: None },
            cascade_hlc,
            cascade_seq: None,
            to_state_version: 2,
        }
    }

    #[test]
    fn roundtrips_with_populated_cascade_hlc() {
        let value = sample(Some(HybridTimestamp::zero()));
        let bytes = to_vec(&value).unwrap();
        let decoded = GroupUpgradeValue::try_from_slice(&bytes).unwrap();
        assert_eq!(decoded.cascade_hlc, Some(HybridTimestamp::zero()));
        assert_eq!(decoded.to_version, "2.0.0");
    }

    #[test]
    fn roundtrips_with_none_cascade_hlc() {
        let value = sample(None);
        let bytes = to_vec(&value).unwrap();
        let decoded = GroupUpgradeValue::try_from_slice(&bytes).unwrap();
        assert_eq!(decoded.cascade_hlc, None);
        assert_eq!(decoded.to_version, "2.0.0");
    }

    #[test]
    fn rejects_partial_cascade_hlc() {
        let mut bytes = Vec::new();
        "1.0.0".to_owned().serialize(&mut bytes).unwrap();
        "2.0.0".to_owned().serialize(&mut bytes).unwrap();
        Some(vec![1u8, 2, 3]).serialize(&mut bytes).unwrap();
        1_700_000_000u64.serialize(&mut bytes).unwrap();
        PrimitivePublicKey::from([7u8; 32])
            .serialize(&mut bytes)
            .unwrap();
        (GroupUpgradeStatus::Completed { completed_at: None })
            .serialize(&mut bytes)
            .unwrap();
        // Push the `Some` tag (0x01) with no HybridTimestamp body — truncated.
        bytes.push(0x01u8);

        let result = GroupUpgradeValue::try_from_slice(&bytes);
        assert!(
            result.is_err(),
            "expected Err for truncated cascade_hlc body"
        );
    }

    #[test]
    fn roundtrips_with_populated_cascade_seq() {
        let mut value = sample(Some(HybridTimestamp::zero()));
        value.cascade_seq = Some(12);
        let bytes = to_vec(&value).unwrap();
        let decoded = GroupUpgradeValue::try_from_slice(&bytes).unwrap();
        assert_eq!(decoded.cascade_hlc, Some(HybridTimestamp::zero()));
        assert_eq!(decoded.cascade_seq, Some(12));
    }

    #[test]
    fn group_upgrade_value_roundtrips_to_state_version() {
        let value = GroupUpgradeValue {
            from_version: "10.1.3".to_owned(),
            to_version: "10.2.0".to_owned(),
            migration: None,
            initiated_at: 7,
            initiated_by: PrimitivePublicKey::from([3; 32]),
            status: GroupUpgradeStatus::InProgress {
                total: 1,
                completed: 0,
                failed: 0,
            },
            cascade_hlc: None,
            cascade_seq: None,
            to_state_version: 2,
        };

        let bytes = to_vec(&value).expect("serialize");
        let back = GroupUpgradeValue::try_from_slice(&bytes).expect("deserialize");

        assert_eq!(back.to_state_version, 2);
        assert_eq!(back.to_version, "10.2.0");
    }

    #[test]
    fn rejects_partial_to_state_version() {
        let mut bytes = to_vec(&sample(None)).unwrap();
        // Two of the four `u32` bytes. Any record short of the full layout must
        // fail loud rather than decode a default.
        bytes.truncate(bytes.len() - 2);

        // Borsh reports short input as `InvalidData`, not `UnexpectedEof`.
        let err = GroupUpgradeValue::try_from_slice(&bytes)
            .expect_err("expected Err for truncated to_state_version");
        assert_eq!(err.kind(), borsh::io::ErrorKind::InvalidData);
    }

    /// The `Completed` layout a shipped binary writes, byte for byte, decoded by
    /// this one. `status` is not the last field, so a field added inside the
    /// variant shifts `cascade_hlc`, `cascade_seq` and `to_state_version` and
    /// every stored record on every already-migrated namespace stops decoding.
    /// Node-local additions go in their own key ([`GroupFleetCompletion`]) so
    /// this stays true.
    #[test]
    fn the_completed_layout_a_shipped_binary_writes_still_decodes() {
        let mut bytes = Vec::new();
        "1.0.0".to_owned().serialize(&mut bytes).unwrap();
        "2.0.0".to_owned().serialize(&mut bytes).unwrap();
        None::<Vec<u8>>.serialize(&mut bytes).unwrap();
        1_700_000_000u64.serialize(&mut bytes).unwrap();
        PrimitivePublicKey::from([7u8; 32])
            .serialize(&mut bytes)
            .unwrap();
        // `Completed`: variant tag, then `completed_at` and nothing else.
        bytes.push(1u8);
        Some(1_700_001_000u64).serialize(&mut bytes).unwrap();
        None::<HybridTimestamp>.serialize(&mut bytes).unwrap();
        None::<u64>.serialize(&mut bytes).unwrap();
        2u32.serialize(&mut bytes).unwrap();

        let decoded = GroupUpgradeValue::try_from_slice(&bytes)
            .expect("a stored Completed record must decode");
        match decoded.status {
            GroupUpgradeStatus::Completed { completed_at } => {
                assert_eq!(completed_at, Some(1_700_001_000));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        // The trailing fields read their own bytes, not shifted ones.
        assert_eq!(decoded.cascade_hlc, None);
        assert_eq!(decoded.cascade_seq, None);
        assert_eq!(decoded.to_state_version, 2);
        assert_eq!(
            to_vec(&decoded).unwrap(),
            bytes,
            "this binary must write back the same bytes it read"
        );
    }
}

/// On-disk backward compatibility for [`GroupKeyValue`].
///
/// `epoch` (#3114) and `insertion_seq` (presence key-ordering fix) were both
/// appended to a struct with live rows on disk. Borsh is untagged, so a derived
/// decode would fail every one of those rows. These tests pin the lenient decode
/// by building the *historical* byte layouts directly — a `[u8; 32]` plus one or
/// two little-endian `u64`s is exactly what borsh emitted for the older shapes —
/// so they keep failing if someone re-derives `BorshDeserialize`.
#[cfg(all(test, feature = "borsh"))]
mod group_key_value_compat_tests {
    use borsh::{to_vec, BorshDeserialize};

    use super::GroupKeyValue;

    const KEY: [u8; 32] = [0xA5; 32];

    /// Layout before #3114: `group_key || created_at`.
    fn v1_bytes(created_at: u64) -> Vec<u8> {
        let mut bytes = KEY.to_vec();
        bytes.extend_from_slice(&created_at.to_le_bytes());
        bytes
    }

    /// Layout after #3114, before `insertion_seq`: `v1 || epoch`.
    fn v2_bytes(created_at: u64, epoch: u64) -> Vec<u8> {
        let mut bytes = v1_bytes(created_at);
        bytes.extend_from_slice(&epoch.to_le_bytes());
        bytes
    }

    #[test]
    fn decodes_pre_epoch_rows_as_genesis() {
        let decoded = GroupKeyValue::try_from_slice(&v1_bytes(1_700_000_000))
            .expect("a pre-epoch row must still decode, not brick the keyring");

        assert_eq!(decoded.group_key, KEY);
        assert_eq!(decoded.created_at, 1_700_000_000);
        assert_eq!(
            decoded.epoch, 0,
            "a row written before epochs is genesis-era"
        );
        assert_eq!(decoded.insertion_seq, 0);
    }

    #[test]
    fn decodes_pre_insertion_seq_rows_and_keeps_the_epoch() {
        let decoded = GroupKeyValue::try_from_slice(&v2_bytes(1_700_000_000, 42))
            .expect("a pre-insertion_seq row must still decode");

        assert_eq!(decoded.epoch, 42, "the epoch that IS on disk must survive");
        assert_eq!(decoded.insertion_seq, 0);
    }

    #[test]
    fn round_trips_the_current_layout() {
        let value = GroupKeyValue {
            group_key: KEY,
            created_at: 7,
            epoch: 9,
            insertion_seq: 11,
        };
        let decoded =
            GroupKeyValue::try_from_slice(&to_vec(&value).expect("serialize")).expect("decode");

        assert_eq!(decoded.group_key, KEY);
        assert_eq!(decoded.created_at, 7);
        assert_eq!(decoded.epoch, 9);
        assert_eq!(decoded.insertion_seq, 11);
    }

    /// Leniency is strictly tail-shaped: a *partial* trailing `u64` is
    /// corruption and must fail loudly rather than decode as a default, and a
    /// buffer missing a field that has always been present is not "old".
    #[test]
    fn rejects_a_partial_trailing_field() {
        let mut bytes = v2_bytes(1, 2);
        bytes.truncate(bytes.len() - 3);

        let _err = GroupKeyValue::try_from_slice(&bytes)
            .expect_err("a half-written u64 must not silently decode as 0");
    }

    #[test]
    fn rejects_a_row_missing_created_at() {
        let _err = GroupKeyValue::try_from_slice(&KEY)
            .expect_err("created_at predates every layout — its absence is corruption");
    }
}
