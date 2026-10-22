use core::convert::Infallible;
use core::fmt::{self, Debug, Formatter};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::application::ApplicationId as PrimitiveApplicationId;
use calimero_primitives::hash::Hash;
use generic_array::sequence::Concat;
use generic_array::typenum::U32;
use generic_array::GenericArray;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};

#[derive(Clone, Copy, Debug)]
pub struct ApplicationId;

impl KeyComponent for ApplicationId {
    type LEN = U32;
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ApplicationMeta(Key<ApplicationId>);

impl ApplicationMeta {
    #[must_use]
    pub fn new(application_id: PrimitiveApplicationId) -> Self {
        Self(Key((*application_id).into()))
    }

    #[must_use]
    pub fn application_id(&self) -> PrimitiveApplicationId {
        (*AsRef::<[_; 32]>::as_ref(&self.0)).into()
    }
}

impl AsKeyParts for ApplicationMeta {
    type Components = (ApplicationId,);

    fn column() -> Column {
        Column::Application
    }

    fn as_key(&self) -> &Key<Self::Components> {
        (&self.0).into()
    }
}

impl FromKeyParts for ApplicationMeta {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(*<&_>::from(&parts)))
    }
}

impl Debug for ApplicationMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Application")
            .field("id", &self.application_id())
            .finish()
    }
}

/// Per-application breadcrumb of the bytecode blob that an in-place install
/// (same id, new bundle version) overwrote. Bundle ApplicationIds are
/// version-stable, so the overwritten blob is otherwise unrecoverable from
/// the row — this is what lets a logically-aborted migration pin its context
/// back to the pre-upgrade code. Own column: the key is application_id-only,
/// same shape as [`ApplicationMeta`], and sharing `Column::Application` would
/// collide with it.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ApplicationPreviousBlob(Key<ApplicationId>);

impl ApplicationPreviousBlob {
    #[must_use]
    pub fn new(application_id: PrimitiveApplicationId) -> Self {
        Self(Key((*application_id).into()))
    }

    #[must_use]
    pub fn application_id(&self) -> PrimitiveApplicationId {
        (*AsRef::<[_; 32]>::as_ref(&self.0)).into()
    }
}

impl AsKeyParts for ApplicationPreviousBlob {
    type Components = (ApplicationId,);

    fn column() -> Column {
        Column::ApplicationPreviousBlob
    }

    fn as_key(&self) -> &Key<Self::Components> {
        (&self.0).into()
    }
}

impl FromKeyParts for ApplicationPreviousBlob {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(*<&_>::from(&parts)))
    }
}

impl Debug for ApplicationPreviousBlob {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApplicationPreviousBlob")
            .field("id", &self.application_id())
            .finish()
    }
}

/// The SHA-256 of a release's version string: a fixed-width stand-in for it in a key.
#[derive(Clone, Copy, Debug)]
pub struct ReleaseVersion;

impl KeyComponent for ReleaseVersion {
    type LEN = U32;
}

/// Which bundle blob holds one release of an application: `application_id(32)
/// || sha256(version)(32)`. A later install of the same release overwrites it.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ApplicationRelease(Key<(ApplicationId, ReleaseVersion)>);

impl ApplicationRelease {
    #[must_use]
    pub fn new(application_id: PrimitiveApplicationId, version: &str) -> Self {
        let digest: [u8; 32] = Hash::new(version.as_bytes()).into();
        Self(Key(
            GenericArray::from(*application_id).concat(GenericArray::from(digest))
        ))
    }
}

impl AsKeyParts for ApplicationRelease {
    type Components = (ApplicationId, ReleaseVersion);

    fn column() -> Column {
        Column::ApplicationRelease
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for ApplicationRelease {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for ApplicationRelease {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApplicationRelease").finish()
    }
}
