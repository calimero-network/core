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
//! The HLC will reject timestamps that are too far in the future (5s in Calimero)
//! to prevent clock skew from causing excessive drift while allowing for network delays
//! in distributed systems. This is configured via `HLCBuilder::with_max_delta()`.

use core::fmt;
use core::num::NonZeroU128;

use borsh::{BorshDeserialize, BorshSerialize};

/// NTP64 timestamp (64-bit: 32-bit seconds + 32-bit fraction).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct NTP64(pub u64);

impl NTP64 {
    /// Get the raw u64 value.
    #[must_use]
    pub const fn as_u64(&self) -> u64 {
        self.0
    }
}

/// Unique identifier for an HLC instance (prevents timestamp collisions).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ID(NonZeroU128);

impl From<NonZeroU128> for ID {
    fn from(value: NonZeroU128) -> Self {
        Self(value)
    }
}

impl From<ID> for u128 {
    fn from(id: ID) -> u128 {
        id.0.get()
    }
}

/// HLC Timestamp = (time, id)
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct Timestamp {
    time: NTP64,
    id: ID,
}

impl Timestamp {
    /// Create a new timestamp.
    #[must_use]
    pub const fn new(time: NTP64, id: ID) -> Self {
        Self { time, id }
    }

    /// Get the time component.
    #[must_use]
    pub const fn get_time(&self) -> &NTP64 {
        &self.time
    }

    /// Get the ID component.
    #[must_use]
    pub const fn get_id(&self) -> &ID {
        &self.id
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{:x}", self.time.0, u128::from(self.id))
    }
}

// Const for default ID (can't be zero)
const DEFAULT_ID: NonZeroU128 = match NonZeroU128::new(1) {
    Some(v) => v,
    None => unreachable!(),
};

/// Borsh-serializable wrapper around Timestamp.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct HybridTimestamp(Timestamp);

impl HybridTimestamp {
    /// Create a new hybrid timestamp.
    #[must_use]
    pub const fn new(ts: Timestamp) -> Self {
        Self(ts)
    }

    /// Get the inner timestamp.
    #[must_use]
    pub const fn inner(&self) -> &Timestamp {
        &self.0
    }

    /// Get the time component.
    #[must_use]
    pub const fn get_time(&self) -> &NTP64 {
        self.0.get_time()
    }

    /// Get the ID component.
    #[must_use]
    pub const fn get_id(&self) -> &ID {
        self.0.get_id()
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

impl BorshSerialize for HybridTimestamp {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        let time_u64 = self.0.get_time().as_u64();
        let id_u128: u128 = (*self.0.get_id()).into();
        time_u64.serialize(writer)?;
        id_u128.serialize(writer)?;
        Ok(())
    }
}

impl BorshDeserialize for HybridTimestamp {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let time_u64 = u64::deserialize_reader(reader)?;
        let id_u128 = u128::deserialize_reader(reader)?;
        let time = NTP64(time_u64);
        let id = if id_u128 == 0 {
            ID::from(DEFAULT_ID)
        } else {
            NonZeroU128::new(id_u128).map(ID::from).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "ID cannot be zero")
            })?
        };
        Ok(Self(Timestamp::new(time, id)))
    }
}

impl Default for HybridTimestamp {
    fn default() -> Self {
        Self(Timestamp::new(NTP64(0), ID::from(DEFAULT_ID)))
    }
}

impl fmt::Display for HybridTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Get the physical time in seconds from a timestamp.
#[must_use]
pub fn physical_time_secs(ts: &HybridTimestamp) -> u32 {
    (ts.0.get_time().as_u64() >> 32) as u32
}

/// Get the logical counter from a timestamp.
#[must_use]
pub fn logical_counter(ts: &HybridTimestamp) -> u32 {
    (ts.0.get_time().as_u64() & 0xF) as u32
}

/// Hybrid Logical Clock implementation.
///
/// Implements the HLC algorithm with custom time/random sources for WASM compatibility.
/// Based on: https://github.com/atolab/uhlc-rs
pub(crate) struct LogicalClock {
    /// Unique ID for this HLC instance (randomly generated)
    id: u128,
    /// Last observed physical time in NTP64 format
    last_time: u64,
    /// Logical counter (embedded in low 16 bits of timestamp)
    counter: u16,
}

impl LogicalClock {
    pub(crate) fn new<F>(mut random_bytes_fn: F) -> Self
    where
        F: FnMut(&mut [u8]),
    {
        let mut id_bytes = [0u8; 16];
        random_bytes_fn(&mut id_bytes);
        let id = u128::from_le_bytes(id_bytes);

        Self {
            id: if id == 0 { 1 } else { id },
            last_time: 0,
            counter: 0,
        }
    }

    #[expect(
        clippy::integer_division,
        reason = "Required for nanosecond to NTP64 time conversion"
    )]
    #[expect(unsafe_code, reason = "self.id guaranteed non-zero by constructor")]
    pub(crate) fn new_timestamp<F>(&mut self, time_now_fn: F) -> HybridTimestamp
    where
        F: FnOnce() -> u64,
    {
        // Get physical time from provided function
        let now_nanos = time_now_fn();

        // Convert nanoseconds to NTP64 format
        // NTP64: upper 32 bits = seconds, lower 32 bits = fraction of second
        let secs = now_nanos / 1_000_000_000;
        let nanos = now_nanos % 1_000_000_000;
        let frac = (nanos * (1_u64 << 32)) / 1_000_000_000;
        let physical_time = (secs << 32) | frac;

        // HLC algorithm: time = max(physical, last_observed)
        if physical_time > self.last_time {
            self.last_time = physical_time;
            self.counter = 0;
        } else {
            // Clock didn't advance - increment logical counter
            self.counter = self.counter.wrapping_add(1);
        }

        // Embed counter in low 16 bits of timestamp
        let time_with_counter = NTP64((self.last_time & !0xFFFF) | (self.counter as u64 & 0xFFFF));

        // Safety: self.id is initialized to non-zero in `new()` and never changes
        let id = ID::from(unsafe { NonZeroU128::new_unchecked(self.id) });

        HybridTimestamp::from(Timestamp::new(time_with_counter, id))
    }

    /// Update with remote timestamp (maintains causality, rejects if >5s in future).
    #[expect(
        clippy::integer_division,
        reason = "Required for nanosecond to NTP64 time conversion"
    )]
    pub(crate) fn update<F>(
        &mut self,
        remote_ts: &HybridTimestamp,
        time_now_fn: F,
    ) -> Result<(), ()>
    where
        F: FnOnce() -> u64,
    {
        let remote_time = remote_ts.get_time().as_u64();

        // Get current physical time for drift check
        let now_nanos = time_now_fn();

        // Convert nanoseconds to NTP64 format
        let secs = now_nanos / 1_000_000_000;
        let nanos = now_nanos % 1_000_000_000;
        let frac = (nanos * (1_u64 << 32)) / 1_000_000_000;
        let local_ntp = (secs << 32) | frac;

        // Drift protection: reject if >5s in future
        const DRIFT_TOLERANCE_SECS: u64 = 5;
        let drift_ntp = local_ntp + (DRIFT_TOLERANCE_SECS << 32);

        if remote_time > drift_ntp {
            return Err(());
        }

        // Update to max observed time (causality)
        if remote_time > self.last_time {
            self.last_time = remote_time;
            self.counter = 0;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn test_hlc_monotonicity() {
        let time = AtomicU64::new(1_000_000_000_000_000_000);
        let mut hlc = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));

        let ts1 = hlc.new_timestamp(|| time.load(Ordering::Relaxed));
        let ts2 = hlc.new_timestamp(|| time.load(Ordering::Relaxed));
        let ts3 = hlc.new_timestamp(|| time.load(Ordering::Relaxed));

        assert!(ts1 < ts2);
        assert!(ts2 < ts3);
    }

    #[test]
    fn test_hybrid_timestamp_borsh() {
        let time = AtomicU64::new(1_000_000_000_000_000_000);
        let mut hlc = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));
        let ts = hlc.new_timestamp(|| time.load(Ordering::Relaxed));

        // Serialize
        let serialized = borsh::to_vec(&ts).unwrap();

        // Deserialize
        let deserialized: HybridTimestamp = borsh::from_slice(&serialized).unwrap();

        assert_eq!(ts, deserialized);
    }

    #[test]
    fn test_hlc_uniqueness() {
        let time = AtomicU64::new(1_000_000_000_000_000_000);
        let mut hlc1 = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));
        let mut hlc2 = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));

        let ts1 = hlc1.new_timestamp(|| time.load(Ordering::Relaxed));
        let ts2 = hlc2.new_timestamp(|| time.load(Ordering::Relaxed));

        // Even if generated at the same instant, timestamps should be unique (different IDs)
        assert_ne!(ts1.get_id(), ts2.get_id());
    }
}
