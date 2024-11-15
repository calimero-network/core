//! A root collection that stores a single value.

use std::cell::RefCell;
use std::ptr;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::env;

use super::Collection;
use crate::address::Id;

/// A set collection that stores unqiue values once.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct Root<T> {
    id: Id,
    inner: Collection<T>,
    value: RefCell<Option<T>>,
}

impl<T> Root<T>
where
    T: BorshSerialize + BorshDeserialize,
{
    /// Creates a new root collection with the given value.
    pub fn new(value: T) -> Self {
        let id = Id::new(env::context_id());

        let mut inner = Collection::new();

        let value = inner.insert(Some(id), value).unwrap();

        Self {
            id,
            inner,
            value: RefCell::new(Some(value)),
        }
    }

    /// Gets the value of the root collection.
    pub fn get(&self) -> &T {
        self.get_mut()
    }

    /// Gets the value of the root collection mutably.
    pub fn get_mut(&self) -> &mut T {
        let mut value = self.value.borrow_mut();

        let value = value.get_or_insert_with(|| self.inner.get(self.id).unwrap().unwrap());

        #[expect(unsafe_code, reason = "necessary for caching")]
        let value = unsafe { &mut *ptr::from_mut(value) };

        value
    }
}
