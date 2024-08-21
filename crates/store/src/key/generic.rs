use std::fmt;

use generic_array::sequence::Concat;
use generic_array::typenum::{U16, U32};
use generic_array::GenericArray;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};

#[derive(Clone, Copy, Debug)]
pub struct Scope;

impl KeyComponent for Scope {
    type LEN = U16;
}

#[derive(Clone, Copy, Debug)]
pub struct Fragment;

impl KeyComponent for Fragment {
    type LEN = U32;
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
pub struct Generic(Key<(Scope, Fragment)>);

impl Generic {
    pub fn new(scope: [u8; 16], fragment: [u8; 32]) -> Self {
        Self(Key(GenericArray::from(scope).concat(fragment.into())))
    }

    pub fn scope(&self) -> [u8; 16] {
        let mut scope = [0; 16];

        scope.copy_from_slice(&AsRef::<[_; 48]>::as_ref(&self.0)[..16]);

        scope
    }

    pub fn fragment(&self) -> [u8; 32] {
        let mut fragment = [0; 32];

        fragment.copy_from_slice(&AsRef::<[_; 48]>::as_ref(&self.0)[16..]);

        fragment
    }
}

impl AsKeyParts for Generic {
    type Components = (Scope, Fragment);

    fn column() -> Column {
        Column::Generic
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for Generic {
    type Error = ();

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl fmt::Debug for Generic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Generic")
            .field("scope", &self.scope())
            .field("fragment", &self.fragment())
            .finish()
    }
}
