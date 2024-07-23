use std::fmt;

use generic_array::typenum::U32;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};

pub struct ApplicationId;

impl KeyComponent for ApplicationId {
    type LEN = U32;
}

#[derive(Eq, Ord, Copy, Clone, PartialEq, PartialOrd)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
pub struct ApplicationMeta(Key<ApplicationId>);

impl ApplicationMeta {
    pub fn new(id: [u8; 32]) -> Self {
        Self(Key(id.into()))
    }

    // todo! define a primitive ApplicationId
    pub fn application_id(&self) -> [u8; 32] {
        *AsRef::<[_; 32]>::as_ref(&self.0)
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
    type Error = ();

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
