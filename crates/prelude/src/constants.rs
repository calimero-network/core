//! Migration and storage constants for Calimero applications.
//!
//! These constants are used for state migration operations in WASM applications.

use sha2::{Digest, Sha256};

/// Re-export of [`calimero_primitives::common::DIGEST_SIZE`] for use in root storage types.
pub use calimero_primitives::common::DIGEST_SIZE;

/// Well-known ID for the root storage entry.
///
/// This is a fixed 32-byte value used as the entry ID for the application's root state.
/// The value `118` represents the ASCII code for 'v' (value), chosen as a memorable constant.
pub const ROOT_STORAGE_ENTRY_ID: [u8; DIGEST_SIZE] = [118u8; DIGEST_SIZE];

/// Computes the storage key of the row holding the root state entry.
///
/// An entity's index record and its data share one row (see [`crate::row`]),
/// stored under the key of its index: SHA-256 of the `Key::Index`
/// discriminant (0x00) followed by the id. Read the state out of that row with
/// [`crate::row::data`].
///
/// This matches `Key::Index(id).to_bytes()` in the storage layer. Both
/// `calimero-sdk` (for `read_raw()` during migrations) and `calimero-storage`
/// use this single implementation to avoid duplication.
#[must_use]
pub fn root_storage_key() -> [u8; DIGEST_SIZE] {
    let mut bytes = [0u8; DIGEST_SIZE + 1];
    bytes[0] = 0; // Key::Index discriminant: the row the entry shares
    bytes[1..DIGEST_SIZE + 1].copy_from_slice(&ROOT_STORAGE_ENTRY_ID);
    Sha256::digest(bytes).into()
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

        assert_eq!(key.len(), DIGEST_SIZE);

        let key2 = root_storage_key();
        assert_eq!(key, key2);

        let mut bytes = [0u8; 33];
        bytes[0] = 0;
        bytes[1..33].copy_from_slice(&ROOT_STORAGE_ENTRY_ID);
        let expected: [u8; 32] = Sha256::digest(bytes).into();
        assert_eq!(key, expected);
    }
}
