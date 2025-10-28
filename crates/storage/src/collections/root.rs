//! A root collection that stores a single value.

use core::fmt;
use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::ptr;

use borsh::{from_slice, BorshDeserialize, BorshSerialize};

use super::{Collection, ROOT_ID};
use crate::address::Id;
use crate::delta::{push_comparison, StorageDelta};
use crate::integration::Comparison;
use crate::interface::{Action, Interface, StorageError};
use crate::store::{MainStorage, StorageAdaptor};

/// A set collection that stores unqiue values once.
pub struct Root<T, S: StorageAdaptor = MainStorage> {
    inner: Collection<T, S>,
    value: RefCell<Option<T>>,
    dirty: bool,
}

impl<T, S> fmt::Debug for Root<T, S>
where
    T: BorshSerialize + BorshDeserialize + fmt::Debug,
    S: StorageAdaptor,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Root")
            .field("inner", &self.inner)
            .field("value", &self.value)
            .field("dirty", &self.dirty)
            .finish()
    }
}

impl<T> Root<T, MainStorage>
where
    T: BorshSerialize + BorshDeserialize,
{
    /// Creates a new root collection with the given value.
    pub fn new<F: FnOnce() -> T>(f: F) -> Self {
        Self::new_internal(f)
    }
}

impl<T, S> Root<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Creates a new root collection with the given value.
    #[expect(clippy::unwrap_used, reason = "fatal error if it happens")]
    pub fn new_internal<F: FnOnce() -> T>(f: F) -> Self {
        let mut inner = Collection::new(Some(*ROOT_ID));

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

    #[expect(clippy::mut_from_ref, reason = "'tis fine")]
    #[expect(clippy::unwrap_used, reason = "fatal error if it happens")]
    fn get(&self) -> &mut T {
        let mut value = self.value.borrow_mut();

        let id = Self::entry_id();

        let value = value.get_or_insert_with(|| self.inner.get(id).unwrap().unwrap());

        #[expect(unsafe_code, reason = "necessary for caching")]
        let value = unsafe { &mut *ptr::from_mut(value) };

        value
    }

    /// Fetches the root collection.
    #[expect(
        clippy::unwrap_used,
        clippy::unwrap_in_result,
        reason = "fatal error if it happens"
    )]
    pub fn fetch() -> Option<Self> {
        let inner = <Interface<S>>::root().unwrap()?;

        Some(Self {
            inner,
            dirty: false,
            value: RefCell::new(None),
        })
    }

    /// Commits the root collection.
    #[expect(clippy::unwrap_used, reason = "fatal error if it happens")]
    pub fn commit(mut self) {
        if self.dirty {
            if let Some(value) = self.value.into_inner() {
                if let Some(mut entry) = self.inner.get_mut(Self::entry_id()).unwrap() {
                    *entry = value;
                }
            }
        }

        <Interface<S>>::commit_root(Some(self.inner)).unwrap();
    }

    /// Commits the root collection without an instance of the root state.
    #[expect(clippy::unwrap_used, reason = "fatal error if it happens")]
    pub fn commit_headless() {
        <Interface<S>>::commit_root::<Collection<T>>(None).unwrap();
    }

    /// Syncs the root collection.
    #[expect(clippy::missing_errors_doc, reason = "NO")]
    pub fn sync(args: &[u8]) -> Result<(), StorageError> {
        let artifact =
            from_slice::<StorageDelta>(args).map_err(StorageError::DeserializationError)?;

        match artifact {
            StorageDelta::Actions(actions) => {
                for action in actions {
                    let _ignored = match action {
                        Action::Compare { id } => {
                            push_comparison(Comparison {
                                data: <Interface<S>>::find_by_id_raw(id),
                                comparison_data: <Interface<S>>::generate_comparison_data(Some(
                                    id,
                                ))?,
                            });
                        }
                        Action::Add { .. } | Action::Update { .. } | Action::DeleteRef { .. } => {
                            <Interface<S>>::apply_action(action)?;
                        }
                    };
                }
            }
            StorageDelta::Comparisons(comparisons) => {
                if comparisons.is_empty() {
                    push_comparison(Comparison {
                        data: <Interface<S>>::find_by_id_raw(Id::root()),
                        comparison_data: <Interface<S>>::generate_comparison_data(None)?,
                    });
                }

                for Comparison {
                    data,
                    comparison_data,
                } in comparisons
                {
                    <Interface<S>>::compare_affective(data, comparison_data)?;
                }
            }
        }

        Self::commit_headless();

        Ok(())
    }
}

impl<T, S> Deref for Root<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl<T, S> DerefMut for Root<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.dirty = true;

        self.get()
    }
}
