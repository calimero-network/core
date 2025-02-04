use std::convert::Infallible;

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::alias::{Alias as AliasPrimitive, Kind as KindPrimitive};
use calimero_primitives::context::ContextId;
use generic_array::sequence::Concat;
use generic_array::typenum::{U1, U32, U50};
use generic_array::GenericArray;

use super::component::{impl_key_components, KeyComponent, KeyComponents};
use super::{AsKeyParts, FromKeyParts, Key};
use crate::db::Column;

//impl_key_components!(Kind, Scope, Alias);

#[derive(Clone, Copy, Debug)]
pub struct Kind;

impl KeyComponent for Kind {
    type LEN = U1;
}
#[derive(Clone, Copy, Debug)]
pub struct Scope;

impl KeyComponent for Scope {
    type LEN = U32;
}

#[derive(Clone, Copy, Debug)]
pub struct Alias;

impl KeyComponent for Alias {
    type LEN = U50;
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct IdentityAlias(Key<(Kind, Scope, Alias)>);

impl IdentityAlias {
    fn create_key(kind: KindPrimitive, scope: [u8; 32], alias: AliasPrimitive) -> Self {
        let kind_array = GenericArray::<u8, U1>::from([kind as u8]);
        let scope_array = GenericArray::<u8, U32>::from(scope);
        let mut alias_array = GenericArray::<u8, U50>::default();
        let alias_str = alias.as_str();
        alias_array[..alias_str.len()].copy_from_slice(alias_str.as_bytes());

        Self(Key(kind_array.concat(scope_array).concat(alias_array)))
    }

    pub fn context(alias: AliasPrimitive) -> Self {
        Self::create_key(KindPrimitive::Context, [0u8; 32], alias)
    }

    pub fn application(alias: AliasPrimitive) -> Self {
        Self::create_key(KindPrimitive::Application, [0u8; 32], alias)
    }

    pub fn identity(context_id: ContextId, alias: AliasPrimitive) -> Self {
        Self::create_key(KindPrimitive::Identity, *context_id, alias)
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
