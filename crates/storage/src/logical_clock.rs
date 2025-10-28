//! Hybrid Logical Clock infrastructure for CRDT ordering.
//!
//! This module wraps the [`uhlc`](https://github.com/atolab/uhlc-rs) crate,
//! which provides production-tested Hybrid Logical Clocks (HLC) used in
//! Eclipse Zenoh.
//!
//! # What is an HLC?
//!
//! A Hybrid Logical Clock combines:
//! - **Physical time** (from system clock) - provides wall-clock semantics
//! - **Logical counter** (embedded in low bits) - provides causal ordering
//!
//! This gives us the best of both worlds:
//! - Timestamps are close to physical time (useful for TTL, debugging)
//! - Timestamps guarantee happens-before relationships (essential for CRDTs)
//! - Immune to clock skew (logical counter ensures monotonicity)
//!
//! # Format
//!
//! Timestamps are 64-bit NTP64 format (RFC-5909):
//! ```text
//! ┌──────────────────────┬──────────────────────┐
//! │   Seconds (32 bits)  │  Fraction (32 bits)  │
//! └──────────────────────┴──────────────────────┘
//!                              └─ Last 4 bits = logical counter
//! ```
//!
//! # Uniqueness
//!
//! Each HLC instance has a unique ID (u128), so timestamps are globally unique
//! across the distributed system without coordination.
//!
//! # Example
//!
//! ```ignore
//! use calimero_storage::env;
//!
//! // Get hybrid timestamp (auto-increments logical clock)
//! let ts = env::hlc_timestamp();
//! println!("Timestamp: {}", ts);
//!
//! // When receiving remote operation, update our clock
//! env::update_hlc(&remote_timestamp);
//! ```
//!
//! # Anti-Drift Protection
//!
//! The HLC will reject timestamps that are too far in the future (500ms by default)
//! to prevent clock skew from causing excessive drift. This is configurable via
//! the `UHLC_MAX_DELTA_MS` environment variable.
//!
//! # Implementation Note
//!
//! HLC instances are created using Calimero's existing `env::random_bytes()`
//! instead of relying on `getrandom`, avoiding additional WASM dependencies
//! (like "js" feature) and ensuring consistent random source across the codebase.
//!
//! # References
//!
//! - [uhlc-rs](https://github.com/atolab/uhlc-rs) - The underlying implementation
//! - [Blog post](http://sergeiturukin.com/2017/06/26/hybrid-logical-clocks.html) - HLC explained
//! - [Original paper](https://cse.buffalo.edu/tech-reports/2014-04.pdf) - Academic foundation

use core::fmt;

use borsh::{BorshDeserialize, BorshSerialize};

// Re-export uhlc types for use throughout the crate
pub use uhlc::{HLCBuilder, Timestamp, HLC, ID, NTP64};

/// Wrapper around uhlc::Timestamp that implements Borsh serialization.
///
/// This allows us to store HLC timestamps in our CRDT storage layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HybridTimestamp(Timestamp);

impl HybridTimestamp {
    /// Create from uhlc Timestamp
    #[must_use]
    pub const fn new(ts: Timestamp) -> Self {
        Self(ts)
    }

    /// Get the underlying uhlc Timestamp
    #[must_use]
    pub const fn inner(&self) -> &Timestamp {
        &self.0
    }

    /// Get the NTP64 time value
    #[must_use]
    pub fn get_time(&self) -> &NTP64 {
        self.0.get_time()
    }

    /// Get the unique ID of the HLC that created this timestamp
    #[must_use]
    pub fn get_id(&self) -> &ID {
        self.0.get_id()
    }

    /// Check if this timestamp is from our HLC
    pub fn is_from(&self, hlc: &HLC) -> bool {
        self.0.get_id() == hlc.get_id()
    }
}

impl From<Timestamp> for HybridTimestamp {
    fn from(ts: Timestamp) -> Self {
        Self(ts)
    }
}

impl From<HybridTimestamp> for Timestamp {
    fn from(ts: HybridTimestamp) -> Self {
        ts.0
    }
}

// Implement Borsh serialization for storage
impl BorshSerialize for HybridTimestamp {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        // Serialize as (NTP64, ID as string)
        let time_u64 = self.0.get_time().as_u64();
        let id_string = format!("{}", self.0.get_id());

        time_u64.serialize(writer)?;
        id_string.serialize(writer)?;
        Ok(())
    }
}

impl BorshDeserialize for HybridTimestamp {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let time_u64 = u64::deserialize_reader(reader)?;
        let id_string = String::deserialize_reader(reader)?;

        let time = NTP64(time_u64);

        // Parse ID from string (format is hex)
        let id_u128 = u128::from_str_radix(&id_string, 16)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        // Use NonZero to create ID, or use a sentinel value if zero
        let id = if id_u128 == 0 {
            ID::from(core::num::NonZeroU128::new(1).unwrap())
        } else {
            ID::from(core::num::NonZeroU128::new(id_u128).unwrap())
        };

        Ok(Self(Timestamp::new(time, id)))
    }
}

impl Default for HybridTimestamp {
    fn default() -> Self {
        // Create a default timestamp with zero time and ID of 1 (can't be zero due to NonZero)
        let id = ID::from(core::num::NonZeroU128::new(1).unwrap());
        Self(Timestamp::new(NTP64(0), id))
    }
}

impl fmt::Display for HybridTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Get the physical time component from a timestamp (seconds since epoch)
#[must_use]
pub fn physical_time_secs(ts: &HybridTimestamp) -> u32 {
    (ts.0.get_time().as_u64() >> 32) as u32
}

/// Get the logical component from a timestamp (embedded in fraction bits)
#[must_use]
pub fn logical_counter(ts: &HybridTimestamp) -> u32 {
    // The logical counter is in the last 4 bits of the fraction
    (ts.0.get_time().as_u64() & 0xF) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hlc_monotonicity() {
        let hlc = HLC::default();

        let ts1 = hlc.new_timestamp();
        let ts2 = hlc.new_timestamp();
        let ts3 = hlc.new_timestamp();

        assert!(ts1 < ts2);
        assert!(ts2 < ts3);
    }

    #[test]
    fn test_hlc_update_with_remote() {
        let hlc1 = HLC::default();
        let hlc2 = HLC::default();

        let _ts1 = hlc1.new_timestamp();
        let ts2 = hlc2.new_timestamp();

        // hlc1 receives ts2 from hlc2
        assert!(hlc1.update_with_timestamp(&ts2).is_ok());

        // Next timestamp from hlc1 should be > ts2
        let ts3 = hlc1.new_timestamp();
        assert!(ts3 > ts2);
    }

    #[test]
    fn test_hlc_anti_drift() {
        let hlc = HLC::default();

        // Create a timestamp far in the future
        let future_time = NTP64(u64::MAX);
        let future_ts = Timestamp::new(future_time, *hlc.get_id());

        // Should reject timestamp that's too far ahead
        assert!(hlc.update_with_timestamp(&future_ts).is_err());
    }

    #[test]
    fn test_hybrid_timestamp_borsh() {
        let hlc = HLC::default();
        let ts = HybridTimestamp::from(hlc.new_timestamp());

        // Serialize
        let serialized = borsh::to_vec(&ts).unwrap();

        // Deserialize
        let deserialized: HybridTimestamp = borsh::from_slice(&serialized).unwrap();

        assert_eq!(ts, deserialized);
    }

    #[test]
    fn test_timestamp_uniqueness() {
        let hlc1 = HLC::default();
        let hlc2 = HLC::default();

        let _ts1 = hlc1.new_timestamp();
        let ts2 = hlc2.new_timestamp();

        // Different HLCs have different IDs
        assert_ne!(hlc1.get_id(), hlc2.get_id());

        // Timestamps from different HLCs are distinguishable
        let ts1_again = hlc1.new_timestamp();
        assert_ne!(ts1_again, ts2);
    }

    #[test]
    fn test_physical_and_logical_components() {
        let hlc = HLC::default();
        let ts1 = HybridTimestamp::from(hlc.new_timestamp());
        let ts2 = HybridTimestamp::from(hlc.new_timestamp());

        // Physical time should be approximately same (within same second)
        assert_eq!(physical_time_secs(&ts1), physical_time_secs(&ts2));

        // But logical counter should increase
        assert!(logical_counter(&ts2) >= logical_counter(&ts1));
    }
}
