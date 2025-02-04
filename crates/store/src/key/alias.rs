use std::convert::Infallible;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::alias::Alias;
use generic_array::sequence::Concat;
use generic_array::typenum::{U1, U32, U400};
use generic_array::GenericArray;

use super::component::KeyComponent;
use super::{AsKeyParts, FromKeyParts, Key};
use crate::db::Column;

#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Context,
    Identity,
    Application,
}

impl KeyComponent for Kind {
    type LEN = U1;
}
#[derive(Clone, Copy, Debug)]
pub struct Scope;

impl KeyComponent for Scope {
    type LEN = U32;
}

impl KeyComponent for Alias {
    type LEN = U400;
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct IdentityAlias(Key<(Kind, Scope, Alias)>);

impl IdentityAlias {
    pub fn new(kind: Kind, scope: [u8; 32], alias: Alias) -> Self {
        let kind_array = GenericArray::<u8, U1>::from([kind as u8]);
        let scope_array = GenericArray::<u8, U32>::from(scope);

        let mut alias_array = GenericArray::<u8, U400>::default();
        let alias_str = alias.as_str();
        alias_array[..alias_str.len()].copy_from_slice(alias_str.as_bytes());

        Self(Key(kind_array.concat(scope_array).concat(alias_array)))
    }
}

impl AsKeyParts for IdentityAlias {
    type Components = (Kind, Scope, Alias);

    fn column() -> Column {
        Column::Alias
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for IdentityAlias {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}
