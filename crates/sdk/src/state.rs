use borsh::{BorshDeserialize, BorshSerialize};
use calimero_prelude::root_storage_key;

use crate::event::AppEvent;

pub trait AppState: BorshSerialize + BorshDeserialize + AppStateInit {
    type Event<'a>: AppEvent + 'a;
}

pub trait Identity<This = Self> {}

impl<T: AppState> Identity<T> for T {}

#[diagnostic::on_unimplemented(
    message = "(calimero)> no method named `#[app::init]` found for type `{Self}`",
    label = "add an `#[app::init]` method to this type"
)]
pub trait AppStateInit: Sized {
    type Return: Identity<Self>;
}

/// Reads the raw bytes of the application's root state from storage.
///
/// This function directly reads the serialized state bytes without deserializing them.
/// It is primarily used during state migrations to access the old state format
/// before transforming it to a new schema.
///
/// # Returns
///
/// * `Some(Vec<u8>)` - The raw serialized state bytes if state exists
/// * `None` - If no state has been stored yet

#[must_use]
pub fn read_raw() -> Option<Vec<u8>> {
    let root_key = root_storage_key();
    crate::env::storage_read(&root_key)
}
