//! Storage configuration constants.

use calimero_prelude::{DIGEST_SIZE, ROOT_STORAGE_ENTRY_ID};
use sha2::{Digest, Sha256};

/// Tombstone retention period in nanoseconds (1 day).
///
/// Tombstones are kept for this duration to enable CRDT conflict resolution
/// and out-of-order message handling. After this period, they are garbage collected.
///
/// Value: 86,400,000,000,000 nanoseconds = 1 day
pub const TOMBSTONE_RETENTION_NANOS: u64 = 86_400_000_000_000;

/// Full resync threshold in nanoseconds (2 days).
///
/// If a node has been offline longer than this threshold, a full resync is
/// triggered instead of incremental sync. This ensures consistency even when
/// tombstones have been garbage collected.
///
/// Value: 172,800,000,000,000 nanoseconds = 2 days
pub const FULL_RESYNC_THRESHOLD_NANOS: u64 = 172_800_000_000_000;

/// Garbage collection interval in nanoseconds (12 hours).
///
/// How often to run garbage collection to clean up old tombstones.
/// Running twice per day keeps storage overhead minimal while being gentle
/// on system resources.
///
/// Value: 43,200,000,000,000 nanoseconds = 12 hours
pub const GC_INTERVAL_NANOS: u64 = 43_200_000_000_000;

/// Drift tolerance in nanoseconds (5 seconds).
///
/// Actions with timestamps further in the future than this tolerance
/// are rejected to prevent Time Drift attacks.
///
/// Value: 5,000,000,000 nanoseconds = 5 seconds
pub const DRIFT_TOLERANCE_NANOS: u64 = 5_000_000_000;

/// Helper: Convert days to nanoseconds.
#[inline]
#[must_use]
pub const fn days_to_nanos(days: u64) -> u64 {
    days * 24 * 60 * 60 * 1_000_000_000
}

/// Helper: Convert hours to nanoseconds.
#[inline]
#[must_use]
pub const fn hours_to_nanos(hours: u64) -> u64 {
    hours * 60 * 60 * 1_000_000_000
}

/// Computes the storage key for the root state entry.
///
/// This function computes the hashed storage key used to store the application's
/// root state. The key is derived by:
/// 1. Creating a 33-byte buffer with the `Key::Entry` discriminant (0x01) as the first byte
/// 2. Copying the `ROOT_STORAGE_ENTRY_ID` into bytes 1-32
/// 3. Hashing the buffer with SHA-256
///
/// This matches the key computation in `Key::Entry(id).to_bytes()` from the storage layer.
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
    fn constants_are_correct() {
        assert_eq!(TOMBSTONE_RETENTION_NANOS, days_to_nanos(1));
        assert_eq!(FULL_RESYNC_THRESHOLD_NANOS, days_to_nanos(2));
        assert_eq!(GC_INTERVAL_NANOS, hours_to_nanos(12));
    }

    #[test]
    fn threshold_is_greater_than_retention() {
        assert!(FULL_RESYNC_THRESHOLD_NANOS > TOMBSTONE_RETENTION_NANOS);
    }

    #[test]
    fn test_root_storage_key() {
        let key = root_storage_key();

        // Verify the key is 32 bytes
        assert_eq!(key.len(), DIGEST_SIZE);

        // Verify the key is deterministic (same input always produces same output)
        let key2 = root_storage_key();
        assert_eq!(key, key2);

        // Verify it matches manual computation
        let mut bytes = [0u8; 33];
        bytes[0] = 1; // Key::Entry discriminant
        bytes[1..33].copy_from_slice(&ROOT_STORAGE_ENTRY_ID);
        let expected: [u8; 32] = Sha256::digest(bytes).into();
        assert_eq!(key, expected);
    }
}
