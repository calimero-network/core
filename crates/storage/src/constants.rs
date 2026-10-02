//! Storage configuration constants.

pub use calimero_prelude::root_storage_key;

/// Drift tolerance in nanoseconds (5 seconds).
///
/// Actions with timestamps further in the future than this tolerance
/// are rejected to prevent Time Drift attacks.
///
/// Value: 5,000,000,000 nanoseconds = 5 seconds
pub const DRIFT_TOLERANCE_NANOS: u64 = 5_000_000_000;
