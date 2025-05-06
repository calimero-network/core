use std::convert::Infallible;
use std::ops::Deref;

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::alias::{Alias as AliasPrimitive, ScopedAlias};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use generic_array::sequence::Concat;
use generic_array::typenum::{U1, U32, U50};
use generic_array::GenericArray;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

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

pub trait Aliasable: ScopedAlias {
    #[doc(hidden)]
    const KIND: u8;
}

#[doc(hidden)]
pub trait ScopeIsh {
    fn as_scope(&self) -> [u8; 32];
    fn from_scope(scope: [u8; 32]) -> Self;
    fn is_default() -> bool {
        false
    }
}

impl<T> ScopeIsh for T
where
    T: From<[u8; 32]> + Deref<Target = [u8; 32]>,
{
    fn as_scope(&self) -> [u8; 32] {
        **self
    }

    fn from_scope(scope: [u8; 32]) -> Self {
        scope.into()
    }
}

#[derive(Copy, Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DefaultScope;

impl ScopeIsh for DefaultScope {
    fn as_scope(&self) -> [u8; 32] {
        [0; 32]
    }

    fn from_scope(_: [u8; 32]) -> Self {
        Self
    }

    fn is_default() -> bool {
        true
    }
}

pub trait StoreScopeCompat {
    type Scope: ScopeIsh;

    fn from_primitives_scope(self) -> Self::Scope;
    fn into_primitives_scope(scope: Self::Scope) -> Self;
}

impl<T: Aliasable + ScopeIsh> StoreScopeCompat for T {
    type Scope = T;

    fn from_primitives_scope(self) -> T {
        self
    }

    fn into_primitives_scope(scope: T) -> T {
        scope
    }
}

impl StoreScopeCompat for () {
    type Scope = DefaultScope;

    fn from_primitives_scope(self) -> DefaultScope {
        DefaultScope
    }

    fn into_primitives_scope(_: DefaultScope) -> () {
        ()
    }
}

impl Aliasable for ContextId {
    const KIND: u8 = 1;
}

impl Aliasable for PublicKey {
    const KIND: u8 = 2;
}

impl Aliasable for ApplicationId {
    const KIND: u8 = 3;
}

impl Alias {
    fn create_key<T: Aliasable>(scope: [u8; 32], alias: [u8; 50]) -> Self {
        let scope = GenericArray::from(scope);
        let alias = GenericArray::from(alias);

        let key = Key(GenericArray::from([T::KIND]).concat(scope).concat(alias));

        Self(key)
    }

    fn scoped<T: Aliasable<Scope: StoreScopeCompat>>(
        scope: Option<T::Scope>,
        strict: bool,
    ) -> Option<[u8; 32]> {
        match scope {
            Some(scope) => Some(scope.from_primitives_scope().as_scope()),
            None if <T::Scope as StoreScopeCompat>::Scope::is_default() || !strict => {
                Some(DefaultScope.as_scope())
            }
            None => None,
        }
    }

    pub fn new<T: Aliasable<Scope: StoreScopeCompat>>(
        scope: Option<T::Scope>,
        alias: AliasPrimitive<T>,
    ) -> Option<Self> {
        let scope = Self::scoped::<T>(scope, true)?;

        let alias_str = alias.as_str();
        let mut alias = [0; 50];
        alias[..alias_str.len()].copy_from_slice(alias_str.as_bytes());

        Some(Self::create_key::<T>(scope, alias))
    }

    #[doc(hidden)]
    pub fn new_unchecked<T: Aliasable<Scope: StoreScopeCompat>>(
        scope: Option<T::Scope>,
        alias: [u8; 50],
    ) -> Self {
        let scope = Self::scoped::<T>(scope, false).expect("unreachable");

        Self::create_key::<T>(scope, alias)
    }

    pub fn scope<T: Aliasable<Scope: StoreScopeCompat>>(&self) -> Option<T::Scope> {
        let bytes = self.0.as_bytes();

        (bytes[0] == PublicKey::KIND).then_some(())?;

        let mut scope = [0; 32];
        scope.copy_from_slice(&bytes[1..33]);

        let scope = <T::Scope as StoreScopeCompat>::Scope::from_scope(scope);

        Some(T::Scope::into_primitives_scope(scope))
    }

    /// Returns the alias if the kind matches the expected kind.
    ///
    /// This also returns `None` if the alias is not valid.
    pub fn alias<T: Aliasable>(&self) -> Option<AliasPrimitive<T>> {
        let bytes = self.0.as_bytes();

        (bytes[0] == T::KIND).then_some(())?;

        let bytes = &bytes[33..];

        let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());

        let name = std::str::from_utf8(&bytes[..len]).ok()?;

        name.parse().ok()
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
