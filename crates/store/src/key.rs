use core::cmp::Ordering;
use core::fmt::{Debug, Formatter};
use core::{fmt, ptr};
#[cfg(feature = "borsh")]
use std::io::{Read, Result as IoResult, Write};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use generic_array::typenum::Const;
use generic_array::{GenericArray, IntoArrayLength};

use crate::db::Column;
use crate::slice::Slice;

mod absorb;
mod alias;
mod application;
mod blobs;
mod component;
mod context;
mod generic;
mod group;

pub use absorb::{AbsorbBufferKey, ABSORB_BUFFER_PREFIX};
pub use alias::{Alias, Aliasable, StoreScopeCompat};
pub use application::{ApplicationMeta, ApplicationPreviousBlob};
pub use blobs::BlobMeta;
pub use calimero_primitives::context::GroupMemberRole;
use component::KeyComponents;
pub use context::{
    ContextActivatedBlob, ContextAuthoredRemaining, ContextConfig, ContextDagDelta,
    ContextExecutingBlob, ContextIdentity, ContextLeftMarker, ContextMeta, ContextMigrationFailed,
    ContextPrivateState, ContextResyncRequested, ContextState, ScopeUnifiedOp,
};
pub use generic::{Generic, FRAGMENT_SIZE, SCOPE_SIZE};
pub use group::{
    AutoFollowFlags, ContextGroupRef, ContextServiceName, ContextServiceNameValue, GroupChildIndex,
    GroupContextIndex, GroupContextMemberCap, GroupContextMetadata, GroupDefaultCaps,
    GroupDefaultCapsValue, GroupDeniedMember, GroupKeyEntry, GroupKeyValue, GroupLocalGovNonce,
    GroupLocalGovNonceWindow, GroupLocalGovNonceWindowValue, GroupMember, GroupMemberCapability,
    GroupMemberCapabilityValue, GroupMemberContext, GroupMemberMetadata, GroupMemberValue,
    GroupMeta, GroupMetaValue, GroupMetadata, GroupOpHead, GroupOpHeadValue, GroupOpLog,
    GroupParentRef, GroupSigningKey, GroupSigningKeyValue, GroupSubgroupVis, GroupSubgroupVisValue,
    GroupUpgradeKey, GroupUpgradeLadder, GroupUpgradeStatus, GroupUpgradeValue, LadderRung,
    NamespaceGovHead, NamespaceGovHeadValue, NamespaceGovOp, NamespaceGovOpValue,
    NamespaceIdentity, NamespaceIdentityValue, PendingSelfPurge, UpgradeLadderValue,
    GROUP_CHILD_INDEX_PREFIX, GROUP_CONTEXT_INDEX_PREFIX, GROUP_CONTEXT_MEMBER_CAP_PREFIX,
    GROUP_CONTEXT_METADATA_PREFIX, GROUP_DEFAULT_CAPS_PREFIX, GROUP_DENIED_MEMBER_PREFIX,
    GROUP_KEY_PREFIX, GROUP_LOCAL_GOV_NONCE_PREFIX, GROUP_LOCAL_GOV_NONCE_WINDOW_PREFIX,
    GROUP_MEMBER_CAPABILITY_PREFIX, GROUP_MEMBER_CONTEXT_PREFIX, GROUP_MEMBER_METADATA_PREFIX,
    GROUP_MEMBER_PREFIX, GROUP_METADATA_PREFIX, GROUP_META_PREFIX, GROUP_OP_HEAD_PREFIX,
    GROUP_OP_LOG_PREFIX, GROUP_PARENT_REF_PREFIX, GROUP_SIGNING_KEY_PREFIX,
    GROUP_SUBGROUP_VIS_PREFIX, GROUP_UPGRADE_PREFIX, NAMESPACE_GOV_HEAD_PREFIX,
    NAMESPACE_GOV_OP_PREFIX, NAMESPACE_IDENTITY_PREFIX, PENDING_SELF_PURGE_PREFIX,
};

// `repr(transparent)` guarantees `Key<T>` shares the layout of its sole field,
// `GenericArray<u8, T::LEN>`. The `&Key<T> <-> &Key<(T,)>` reference casts below
// rely on this: both are transparent wrappers over the same array type (the
// `(T,): KeyComponents<LEN = T::LEN>` bound forces equal lengths), so the two
// have identical layout and the pointer cast is sound. Without this attribute
// the layout of a `repr(Rust)` struct is unspecified and the casts would be UB.
#[repr(transparent)]
pub struct Key<T: KeyComponents>(GenericArray<u8, T::LEN>);

impl<T: KeyComponents> Copy for Key<T> where GenericArray<u8, T::LEN>: Copy {}
impl<T: KeyComponents> Clone for Key<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: KeyComponents> Debug for Key<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Key").field(&self.0).finish()
    }
}

impl<T: KeyComponents> Eq for Key<T> where GenericArray<u8, T::LEN>: Eq {}
impl<T: KeyComponents> PartialEq for Key<T>
where
    GenericArray<u8, T::LEN>: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: KeyComponents> Ord for Key<T>
where
    GenericArray<u8, T::LEN>: Ord,
{
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl<T: KeyComponents> PartialOrd for Key<T>
where
    GenericArray<u8, T::LEN>: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: KeyComponents> Key<T> {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn as_slice(&self) -> Slice<'_> {
        self.as_bytes().into()
    }

    #[must_use]
    pub const fn len() -> usize {
        GenericArray::<u8, T::LEN>::len()
    }

    #[must_use]
    pub fn try_from_slice(slice: &[u8]) -> Option<Self> {
        #[expect(
            clippy::use_self,
            reason = "Needed here in order to specify type parameter"
        )]
        (slice.len() == Key::<T>::len()).then_some(())?;

        let mut key = GenericArray::default();

        key.copy_from_slice(slice);

        Some(Self(key))
    }
}

impl<T: KeyComponents, const N: usize> AsRef<[u8; N]> for Key<T>
where
    Const<N>: IntoArrayLength<ArrayLength = T::LEN>,
{
    fn as_ref(&self) -> &[u8; N] {
        self.0.as_ref()
    }
}

impl<T> From<&Key<T>> for &Key<(T,)>
where
    T: KeyComponents,
    (T,): KeyComponents<LEN = T::LEN>,
{
    fn from(key: &Key<T>) -> Self {
        // SAFETY: `Key<T>` and `Key<(T,)>` are both `#[repr(transparent)]` over
        // `GenericArray<u8, T::LEN>` (the `LEN = T::LEN` bound makes the arrays
        // identical), so they share layout and this reference cast is sound.
        unsafe { &*ptr::from_ref(key).cast() }
    }
}

impl<T> From<&Key<(T,)>> for &Key<T>
where
    T: KeyComponents,
    (T,): KeyComponents<LEN = T::LEN>,
{
    fn from(key: &Key<(T,)>) -> Self {
        // SAFETY: see the inverse impl above — identical `repr(transparent)`
        // layout over `GenericArray<u8, T::LEN>` makes this cast sound.
        unsafe { &*ptr::from_ref(key).cast() }
    }
}

pub trait AsKeyParts: Copy + 'static {
    // KeyParts is Sealed so far as KeyComponents stays private
    type Components: KeyComponents;

    fn column() -> Column;
    fn as_key(&self) -> &Key<Self::Components>;
}

pub trait FromKeyParts: AsKeyParts {
    type Error;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error>;
}

#[cfg(feature = "borsh")]
const _: () = {
    impl<T: KeyComponents> BorshSerialize for Key<T> {
        fn serialize<W: Write>(&self, writer: &mut W) -> IoResult<()> {
            writer.write_all(&self.0)
        }
    }

    impl<T: KeyComponents> BorshDeserialize for Key<T> {
        fn deserialize_reader<R: Read>(reader: &mut R) -> IoResult<Self> {
            let mut key = GenericArray::default();

            reader.read_exact(&mut key)?;

            Ok(Self(key))
        }
    }
};
