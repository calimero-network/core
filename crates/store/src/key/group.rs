use core::convert::Infallible;
use core::fmt::{self, Debug, Formatter};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId as PrimitiveContextId, UpgradePolicy};
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

/// Stored against [`GroupMeta`]. Captures the immutable + mutable metadata of a
/// context group.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupMetaValue {
    pub app_key: [u8; 32],
    pub target_application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub created_at: u64,
    pub admin_identity: PrimitivePublicKey,
}

/// Tracks the progress of a group-wide upgrade operation.
/// Stored against [`GroupUpgradeKey`].
///
/// `ApplicationId` is stable across versions (`hash(package, signer_id)`), so
/// upgrades are tracked by revision number, not by application id.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupUpgradeValue {
    pub from_revision: u64,
    pub to_revision: u64,
    pub migration: Option<Vec<u8>>,
    pub initiated_at: u64,
    pub initiated_by: PrimitivePublicKey,
    pub status: GroupUpgradeStatus,
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
        completed_at: u64,
    },
    RolledBack {
        reason: String,
    },
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

    #[cfg(feature = "borsh")]
    mod value_roundtrips {
        use borsh::{from_slice, to_vec};
        use calimero_primitives::application::ApplicationId;
        use calimero_primitives::context::{GroupMemberRole, UpgradePolicy};
        use calimero_primitives::identity::PublicKey as PrimitivePublicKey;

        use super::super::{GroupMetaValue, GroupUpgradeStatus, GroupUpgradeValue};


        #[test]
        fn group_meta_value_roundtrip() {
            let value = GroupMetaValue {
                app_key: [0xAA; 32],
                target_application_id: ApplicationId::from([0xBB; 32]),
                upgrade_policy: UpgradePolicy::Automatic,
                created_at: 1_700_000_000,
                admin_identity: PrimitivePublicKey::from([0xCC; 32]),
            };

            let bytes = to_vec(&value).expect("serialize");
            let decoded: GroupMetaValue = from_slice(&bytes).expect("deserialize");

            assert_eq!(decoded.app_key, value.app_key);
            assert_eq!(decoded.target_application_id, value.target_application_id);
            assert_eq!(decoded.created_at, value.created_at);
            assert_eq!(decoded.admin_identity, value.admin_identity);
            assert!(matches!(decoded.upgrade_policy, UpgradePolicy::Automatic));
        }

        #[test]
        fn group_meta_value_coordinated_policy_roundtrip() {
            use core::time::Duration;

            let value = GroupMetaValue {
                app_key: [0x11; 32],
                target_application_id: ApplicationId::from([0x22; 32]),
                upgrade_policy: UpgradePolicy::Coordinated {
                    deadline: Some(Duration::from_secs(3600)),
                },
                created_at: 1_700_000_000,
                admin_identity: PrimitivePublicKey::from([0x33; 32]),
            };

            let bytes = to_vec(&value).expect("serialize");
            let decoded: GroupMetaValue = from_slice(&bytes).expect("deserialize");

            match decoded.upgrade_policy {
                UpgradePolicy::Coordinated { deadline } => {
                    assert_eq!(deadline, Some(Duration::from_secs(3600)));
                }
                other => panic!("expected Coordinated, got {other:?}"),
            }
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
                from_revision: 1,
                to_revision: 2,
                migration: Some(vec![0xDE, 0xAD]),
                initiated_at: 1_700_000_000,
                initiated_by: PrimitivePublicKey::from([0x03; 32]),
                status: GroupUpgradeStatus::InProgress {
                    total: 10,
                    completed: 3,
                    failed: 1,
                },
            };

            let bytes = to_vec(&value).expect("serialize");
            let decoded: GroupUpgradeValue = from_slice(&bytes).expect("deserialize");

            assert_eq!(decoded.from_revision, 1);
            assert_eq!(decoded.to_revision, 2);
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
        }

        #[test]
        fn group_upgrade_value_no_migration_roundtrip() {
            let value = GroupUpgradeValue {
                from_revision: 3,
                to_revision: 4,
                migration: None,
                initiated_at: 1_700_000_000,
                initiated_by: PrimitivePublicKey::from([0x06; 32]),
                status: GroupUpgradeStatus::Completed {
                    completed_at: 1_700_001_000,
                },
            };

            let bytes = to_vec(&value).expect("serialize");
            let decoded: GroupUpgradeValue = from_slice(&bytes).expect("deserialize");

            assert_eq!(decoded.from_revision, 3);
            assert_eq!(decoded.to_revision, 4);
            assert_eq!(decoded.migration, None);
            match decoded.status {
                GroupUpgradeStatus::Completed { completed_at } => {
                    assert_eq!(completed_at, 1_700_001_000);
                }
                other => panic!("expected Completed, got {other:?}"),
            }
        }

        #[test]
        fn group_upgrade_status_rolled_back_roundtrip() {
            let status = GroupUpgradeStatus::RolledBack {
                reason: "canary failed".to_owned(),
            };

            let bytes = to_vec(&status).expect("serialize");
            let decoded: GroupUpgradeStatus = from_slice(&bytes).expect("deserialize");

            match decoded {
                GroupUpgradeStatus::RolledBack { reason } => {
                    assert_eq!(reason, "canary failed");
                }
                other => panic!("expected RolledBack, got {other:?}"),
            }
        }
    }
}
