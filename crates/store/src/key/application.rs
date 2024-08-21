use std::convert::Infallible;
use std::fmt;

use generic_array::typenum::U32;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};

pub struct ApplicationId;

impl KeyComponent for ApplicationId {
    type LEN = U32;
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
pub struct ApplicationMeta(Key<ApplicationId>);

impl ApplicationMeta {
    pub fn new(application_id: calimero_primitives::application::ApplicationId) -> Self {
        Self(Key((*application_id).into()))
    }

    pub fn application_id(&self) -> calimero_primitives::application::ApplicationId {
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

impl fmt::Debug for ApplicationMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Application")
            .field("id", &self.application_id())
            .finish()
    }
}
