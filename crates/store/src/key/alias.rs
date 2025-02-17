use std::convert::Infallible;

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::alias::{Alias as AliasPrimitive, Kind as KindPrimitive};
use calimero_primitives::context::ContextId;
use generic_array::sequence::Concat;
use generic_array::typenum::{U1, U32, U50};
use generic_array::GenericArray;

use super::component::KeyComponent;
use super::{AsKeyParts, FromKeyParts, Key};
use crate::db::Column;

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
pub struct Name;

impl KeyComponent for Name {
    type LEN = U50;
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct Alias(Key<(Kind, Scope, Name)>);

impl Alias {
    fn create_key(kind: KindPrimitive, scope: [u8; 32], alias: AliasPrimitive) -> Self {
        let kind: u8 = match kind {
            KindPrimitive::Context => 1,
            KindPrimitive::Identity => 2,
            KindPrimitive::Application => 3,
        };
        let kind_array = GenericArray::<u8, U1>::from([kind]);
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

    pub fn kind(&self) -> KindPrimitive {
        match AsRef::<[_; 83]>::as_ref(&self.0)[0] {
            1 => KindPrimitive::Context,
            2 => KindPrimitive::Identity,
            3 => KindPrimitive::Application,
            _ => panic!("invalid kind"),
        }
    }

    pub fn scope(&self) -> [u8; 32] {
        let mut scope = [0; 32];
        scope.copy_from_slice(&AsRef::<[_; 83]>::as_ref(&self.0)[1..33]);
        scope
    }

    pub fn alias(&self) -> AliasPrimitive {
        let mut alias = [0; 50];
        alias.copy_from_slice(&AsRef::<[_; 83]>::as_ref(&self.0)[33..]);

        String::from_utf8(alias.to_vec())
            .expect("valid utf-8")
            .try_into()
            .expect("alias length already validated during construction")
    }
}

impl AsKeyParts for Alias {
    type Components = (Kind, Scope, Name);

    fn column() -> Column {
        Column::Alias
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for Alias {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}
