use core::convert::Infallible;
use core::fmt::{self, Debug, Formatter};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::context::ContextId as PrimitiveContextId;
use calimero_primitives::identity::PublicKey as PrimitivePublicKey;
use generic_array::sequence::Concat;
use generic_array::typenum::{U1, U32};
use generic_array::GenericArray;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};

const GROUP_META_PREFIX: u8 = 0x20;
const GROUP_MEMBER_PREFIX: u8 = 0x21;
const GROUP_CONTEXT_INDEX_PREFIX: u8 = 0x22;
const CONTEXT_GROUP_REF_PREFIX: u8 = 0x23;
const GROUP_UPGRADE_PREFIX: u8 = 0x24;

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
            GenericArray::from([GROUP_META_PREFIX]).concat(GenericArray::from(group_id)),
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
        Column::Config
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
    #[must_use]
    pub fn new(group_id: [u8; 32], identity: PrimitivePublicKey) -> Self {
        Self(Key(
            GenericArray::from([GROUP_MEMBER_PREFIX])
                .concat(GenericArray::from(group_id))
                .concat(GenericArray::from(*identity)),
        ))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn identity(&self) -> PrimitivePublicKey {
        let mut pk = [0; 32];
        pk.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        pk.into()
    }
}

impl AsKeyParts for GroupMember {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Config
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
            .field("identity", &self.identity())
            .finish()
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupContextIndex(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupContextIndex {
    #[must_use]
    pub fn new(group_id: [u8; 32], context_id: PrimitiveContextId) -> Self {
        Self(Key(
            GenericArray::from([GROUP_CONTEXT_INDEX_PREFIX])
                .concat(GenericArray::from(group_id))
                .concat(GenericArray::from(*context_id)),
        ))
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
        Column::Config
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
            GenericArray::from([CONTEXT_GROUP_REF_PREFIX]).concat(GenericArray::from(*context_id)),
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
        Column::Config
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
            GenericArray::from([GROUP_UPGRADE_PREFIX]).concat(GenericArray::from(group_id)),
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
        Column::Config
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

#[cfg(test)]
mod tests {
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
        let pk = PrimitivePublicKey::from([0xEF; 32]);
        let key = GroupMember::new(gid, pk);
        assert_eq!(key.group_id(), gid);
        assert_eq!(key.identity(), pk);
        assert_eq!(key.as_key().as_bytes()[0], GROUP_MEMBER_PREFIX);
        assert_eq!(key.as_key().as_bytes().len(), 65);
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
    fn distinct_prefixes() {
        let prefixes = [
            GROUP_META_PREFIX,
            GROUP_MEMBER_PREFIX,
            GROUP_CONTEXT_INDEX_PREFIX,
            CONTEXT_GROUP_REF_PREFIX,
            GROUP_UPGRADE_PREFIX,
        ];
        for i in 0..prefixes.len() {
            for j in (i + 1)..prefixes.len() {
                assert_ne!(prefixes[i], prefixes[j], "prefix collision at indices {i} and {j}");
            }
        }
    }
}
