//! A root collection that stores a single value.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::ptr;

use borsh::{BorshDeserialize, BorshSerialize};

use super::{Collection, Entry};
use crate::address::Id;
use crate::entities::Data;
use crate::env;

/// Thing.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct RootHandle<T> {
    /// The ID of the root collection.
    pub id: Id,
    _priv: PhantomData<T>,
}

impl<T: Data> RootHandle<T> {
    fn new(id: Id) -> Self {
        Self {
            id,
            _priv: PhantomData,
        }
    }
}

impl<T: Data> crate::entities::Collection for RootHandle<T> {
    type Child = T;

    fn name(&self) -> &str {
        "RootHandle"
    }
}

thread_local! {
    /// The root collection handle.
    pub static ROOT: RefCell<Option<RootHandle<Entry<()>>>> = RefCell::new(None);
}

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

        let old = ROOT.with(|root| root.borrow_mut().replace(RootHandle::new(id)));

        if old.is_some() {
            panic!("root collection already defined");
        }

        let mut inner = Collection::new(Some(id));

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
