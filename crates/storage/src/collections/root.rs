//! A root collection that stores a single value.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::sync::LazyLock;

use borsh::{BorshDeserialize, BorshSerialize};

use super::{Collection, Entry};
use crate::address::Id;
use crate::entities::Data;
use crate::env;
use crate::interface::Interface;

/// Thing.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) struct RootHandle<T> {
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

static ID: LazyLock<Id> = LazyLock::new(|| Id::new(env::context_id()));

/// Prepares the root collection.
fn employ_root_guard() {
    let old = ROOT.with(|root| root.borrow_mut().replace(RootHandle::new(*ID)));

    if old.is_some() {
        panic!("root collection already defined");
    }
}

/// A set collection that stores unqiue values once.
#[derive(Debug)]
pub struct Root<T> {
    inner: Collection<T>,
    value: RefCell<Option<T>>,
    dirty: bool,
}

impl<T> Root<T>
where
    T: BorshSerialize + BorshDeserialize,
{
    /// Creates a new root collection with the given value.
    pub fn new<F: FnOnce() -> T>(f: F) -> Self {
        employ_root_guard();

        let mut inner = Collection::new(Some(*ID));

        let id = Self::entry_id();

        let value = inner.insert(Some(id), f()).unwrap();

        Self {
            inner,
            dirty: false,
            value: RefCell::new(Some(value)),
        }
    }

    fn entry_id() -> Id {
        Id::new([118; 32])
    }

    fn get(&self) -> &mut T {
        let mut value = self.value.borrow_mut();

        let id = Self::entry_id();

        let value = value.get_or_insert_with(|| self.inner.get(id).unwrap().unwrap());

        #[expect(unsafe_code, reason = "necessary for caching")]
        let value = unsafe { &mut *ptr::from_mut(value) };

        value
    }

    /// Fetches the root collection.
    pub fn fetch() -> Option<Self> {
        let inner = Interface::root().unwrap()?;

        employ_root_guard();

        Some(Self {
            inner,
            dirty: false,
            value: RefCell::new(None),
        })
    }

    /// Commits the root collection.
    pub fn commit(mut self) {
        let _ignored = ROOT.with(|root| root.borrow_mut().take());

        if self.dirty {
            if let Some(value) = self.value.into_inner() {
                if let Some(mut entry) = self.inner.get_mut(Self::entry_id()).unwrap() {
                    *entry = value;
                }
            }
        }

        Interface::commit_root(self.inner).unwrap();
    }
}

impl<T> Deref for Root<T>
where
    T: BorshSerialize + BorshDeserialize,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl<T> DerefMut for Root<T>
where
    T: BorshSerialize + BorshDeserialize,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.dirty = true;

        self.get()
    }
}
