use core::convert::Infallible;
use core::fmt::{self, Debug, Formatter};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::application::ApplicationId as PrimitiveApplicationId;
use generic_array::typenum::U32;

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
