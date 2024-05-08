use generic_array::GenericArray;

use crate::db::Column;
use crate::slice::Slice;

mod component;
mod context;

use component::KeyComponents;
pub use context::{ContextIdentity, ContextMembers, ContextState, ContextTransaction};

pub struct Key<T: KeyComponents>(GenericArray<u8, T::LEN>);

impl<T: KeyComponents> Copy for Key<T> where GenericArray<u8, T::LEN>: Copy {}

impl<T: KeyComponents> Clone for Key<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: KeyComponents> Key<T> {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn as_slice(&self) -> Slice {
        self.as_bytes().into()
    }
}

impl<T> From<&Key<T>> for &Key<(T,)>
where
    T: KeyComponents,
    (T,): KeyComponents<LEN = <T as KeyComponents>::LEN>,
{
    fn from(key: &Key<T>) -> Self {
        unsafe { &*(key as *const _ as *const _) }
    }
}

pub trait KeyParts: Copy {
    // KeyParts is Sealed so far as KeyComponents stays private
    type Components: KeyComponents;

    fn column(&self) -> Column;
    fn key(&self) -> &Key<Self::Components>;
}
