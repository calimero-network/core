//! Migration and storage constants for Calimero applications.
//!
//! These constants are used for state migration operations in WASM applications.

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
}
