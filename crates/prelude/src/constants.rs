//! Migration and storage constants for Calimero applications.
//!
//! These constants are used for state migration operations in WASM applications.

/// Re-export of [`calimero_primitives::common::DIGEST_SIZE`] for use in root storage types.
pub use calimero_primitives::common::DIGEST_SIZE;

/// Well-known ID for the root storage entry.
///
/// This is a fixed 32-byte value used as the entry ID for the application's root state.
/// The value `118` represents the ASCII code for 'v' (value), chosen as a memorable constant.
pub const ROOT_STORAGE_ENTRY_ID: [u8; DIGEST_SIZE] = [118u8; DIGEST_SIZE];

/// Length of a context-state key: a one-byte kind tag followed by the id it
/// is about.
pub const STATE_KEY_LEN: usize = DIGEST_SIZE + 1;

/// Kind tag of an entity row's key (`Key::Index` in the storage layer).
pub const ENTITY_KEY_TAG: u8 = 0;

/// Kind tag of an `#[app::private]` blob's key in node-local private state.
///
/// Private state holds both the storage layer's rows (tagged like shared
/// state) and these blobs, so the tag keeps a blob clear of every storage
/// key kind. Far from the storage layer's small tags, so a new kind there
/// does not meet it.
pub const PRIVATE_BLOB_KEY_TAG: u8 = 0xFF;

/// The storage key of the row holding the root state entry.
///
/// An entity's index record and its data share one row (see [`crate::row`]),
/// stored under `ENTITY_KEY_TAG ‖ id`. Read the state out of that row with
/// [`crate::row::data`], passing [`ROOT_STORAGE_ENTRY_ID`].
///
/// This matches `Key::Index(id).to_bytes()` in the storage layer. Both
/// `calimero-sdk` (for `read_raw()` during migrations) and `calimero-storage`
/// use this single implementation to avoid duplication.
#[must_use]
pub fn root_storage_key() -> [u8; STATE_KEY_LEN] {
    let mut key = [0u8; STATE_KEY_LEN];
    key[0] = ENTITY_KEY_TAG;
    key[1..].copy_from_slice(&ROOT_STORAGE_ENTRY_ID);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_size_is_32() {
        assert_eq!(DIGEST_SIZE, 32);
    }

    #[test]
    fn root_storage_entry_id_is_correct() {
        assert_eq!(ROOT_STORAGE_ENTRY_ID.len(), DIGEST_SIZE);
        assert!(ROOT_STORAGE_ENTRY_ID.iter().all(|&b| b == 118u8));
    }

    #[test]
    fn test_root_storage_key() {
        let key = root_storage_key();
        assert_eq!(key.len(), STATE_KEY_LEN);
        assert_eq!(key[0], ENTITY_KEY_TAG);
        assert_eq!(key[1..], ROOT_STORAGE_ENTRY_ID);
    }
}
