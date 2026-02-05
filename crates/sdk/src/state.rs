use borsh::{BorshDeserialize, BorshSerialize};
use calimero_prelude::{DIGEST_SIZE, ROOT_STORAGE_ENTRY_ID};
use sha2::{Digest, Sha256};

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
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::state::read_raw;
///
/// // Read raw state bytes for migration
/// if let Some(old_state_bytes) = read_raw() {
///     // Deserialize old format and migrate to new format
///     let old_state: OldState = borsh::from_slice(&old_state_bytes).unwrap();
///     let new_state = migrate_state(old_state);
/// }
/// ```
#[must_use]
pub fn read_raw() -> Option<Vec<u8>> {
    // Compute the storage key for the root state entry.
    // This matches the key computation in `Key::Entry(id).to_bytes()` from calimero-storage.
    let root_key: [u8; DIGEST_SIZE] = {
        let mut bytes = [0u8; DIGEST_SIZE + 1];
        bytes[0] = 1; // Key::Entry discriminant
        bytes[1..DIGEST_SIZE + 1].copy_from_slice(&ROOT_STORAGE_ENTRY_ID);
        Sha256::digest(bytes).into()
    };

    crate::env::storage_read(&root_key)
}
