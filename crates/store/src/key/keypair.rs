use core::convert::Infallible;
use core::fmt::{self, Debug, Formatter};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey as PrimitivePublicKey;
use generic_array::typenum::U32;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};

#[derive(Clone, Copy, Debug)]
pub struct PublicKey;

impl KeyComponent for PublicKey {
    type LEN = U32;
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct Keypair(Key<PublicKey>);

impl Keypair {
    #[must_use]
    pub fn new(public_key: PrimitivePublicKey) -> Self {
        Self(Key((*public_key).into()))
    }

    #[must_use]
    pub fn public_key(&self) -> PrimitivePublicKey {
        (*AsRef::<[_; 32]>::as_ref(&self.0)).into()
    }
}

impl AsKeyParts for Keypair {
    type Components = (PublicKey,);

    fn column() -> Column {
        Column::Keypairs
    }

    fn as_key(&self) -> &Key<Self::Components> {
        (&self.0).into()
    }
}

impl FromKeyParts for Keypair {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(*<&_>::from(&parts)))
    }
}

impl Debug for Keypair {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keypair")
            .field("public_key", &self.public_key())
            .finish()
    }
}
