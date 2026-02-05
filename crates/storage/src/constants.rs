//! Storage configuration constants.

pub use calimero_prelude::root_storage_key;

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
}
