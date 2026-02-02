use borsh::{BorshDeserialize, BorshSerialize};

use crate::env;
use crate::event::AppEvent;
use calimero_prelude::{DIGEST_SIZE, ROOT_STORAGE_ENTRY_ID};
use sha2::{Digest, Sha256};

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

/// Reads the raw bytes of the root application state.
///
/// This is intended for use within `#[app::migrate]` functions to bypass
/// loading and deserialization of the app state. This allows reading the raw bytes
/// from the previous version of app state.
///
/// NOTE: we are not using the implementation of `storage::constants::root_storage_key()`
/// to avoid getting a redundant dependency of `calimero-storage` in SDK crate.
pub fn read_raw() -> Option<Vec<u8>> {
    // The raw storage key used to access the Root state in the store layer.
    //
    // It corresponds to `Key::Entry(Id::new(ROOT_ENTRY_ID))`.
    // The calculation is the following: SHA256([1u8] + [118u8; 32])
    let root_key: [u8; DIGEST_SIZE] = {
        let mut bytes = [0u8; DIGEST_SIZE + 1];

        // 1 is the discriminant for Key::Entry in `crates/storage/src/store.rs`
        bytes[0] = 1;

        // Copy to the rest the root entry ID
        bytes[1..DIGEST_SIZE + 1].copy_from_slice(&ROOT_STORAGE_ENTRY_ID);

        // Compute the hash that is internally used in RocksDB as the key
        Sha256::digest(bytes).into()
    };

    env::storage_read(&root_key)
}
