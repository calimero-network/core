//! Migration and storage constants for Calimero applications.
//!
//! These constants are used for state migration operations in WASM applications.

use sha2::{Digest, Sha256};

/// Size of cryptographic digests (SHA-256), in bytes.
pub const DIGEST_SIZE: usize = 32;

/// Well-known ID for the root storage entry.
///
/// This is a fixed 32-byte value used as the entry ID for the application's root state.
/// The value `118` represents the ASCII code for 'v' (value), chosen as a memorable constant.
///
/// The actual storage key is computed by hashing this ID with the `Key::Entry` discriminant
/// prefix, resulting in a unique key for the root state entry.
pub const ROOT_STORAGE_ENTRY_ID: [u8; DIGEST_SIZE] = [118u8; DIGEST_SIZE];

/// Computes the storage key for the root state entry.
///
/// This function computes the hashed storage key used to store the application's
/// root state. The key is derived by:
/// 1. Creating a 33-byte buffer with the `Key::Entry` discriminant (0x01) as the first byte
/// 2. Copying the `ROOT_STORAGE_ENTRY_ID` into bytes 1-32
/// 3. Hashing the buffer with SHA-256
///
/// This matches the key computation in `Key::Entry(id).to_bytes()` from the storage layer.
/// Both `calimero-sdk` (for `read_raw()` during migrations) and `calimero-storage` use this
/// single implementation to avoid duplication.
#[must_use]
pub fn root_storage_key() -> [u8; DIGEST_SIZE] {
    let mut bytes = [0u8; DIGEST_SIZE + 1];
    bytes[0] = 1; // Key::Entry discriminant
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
        bytes[0] = 1;
        bytes[1..33].copy_from_slice(&ROOT_STORAGE_ENTRY_ID);
        let expected: [u8; 32] = Sha256::digest(bytes).into();
        assert_eq!(key, expected);
    }
}
