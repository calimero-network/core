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
/// The storage layer wraps user data in an `Entry<T>` envelope that appends a
/// 32-byte `Element.id` suffix after the Borsh-serialized user struct. This
/// function strips that suffix so callers receive only the user data portion,
/// matching the layout of the user's `#[app::state]` struct.
///
/// # Returns
///
/// * `Some(Vec<u8>)` - The raw serialized state bytes (user data only) if state exists
/// * `None` - If no state has been stored yet

#[must_use]
pub fn read_raw() -> Option<Vec<u8>> {
    let root_key = root_storage_key();
    let bytes = crate::env::storage_read(&root_key)?;

    // The storage layer stores entities as Entry<T> = borsh(T) ++ borsh(Element.id).
    // Element only serializes its `id: Id` field ([u8; 32]), all other fields are
    // #[borsh(skip)]. Strip this 32-byte suffix so migration code sees only the
    // user's state struct bytes. Use >= so that when user state is 0 bytes (entry
    // is exactly 32 bytes) we strip the suffix and return an empty Vec, not the id.
    const ELEMENT_SUFFIX_LEN: usize = 32;
    if bytes.len() >= ELEMENT_SUFFIX_LEN {
        Some(bytes[..bytes.len() - ELEMENT_SUFFIX_LEN].to_vec())
    } else {
        Some(bytes)
    }
}
