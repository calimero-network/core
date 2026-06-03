//! Hybrid Logical Clock infrastructure for CRDT ordering.
//!
//! This module provides a self-contained Hybrid Logical Clock (HLC), modeled on
//! the [`uhlc`](https://github.com/atolab/uhlc-rs) crate used in Eclipse Zenoh,
//! but with pluggable time and randomness sources so it also runs inside the
//! deterministic WASM guest environment (no access to the system clock or RNG).
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
//!                                   └─ Low 16 bits = logical counter
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
    /// Zero timestamp for deterministic initialization during merge.
    #[must_use]
    pub fn zero() -> Self {
        Self(Timestamp::new(NTP64(0), ID::from(DEFAULT_ID)))
    }

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

/// Number of low bits in an NTP64 timestamp reserved for the HLC logical
/// counter. Defining the layout once keeps every embed/extract site in sync
/// (a stray mask here previously truncated the counter to 4 bits).
const COUNTER_BITS: u64 = 16;
/// Mask selecting the logical-counter bits of an NTP64 timestamp.
const COUNTER_MASK: u64 = (1 << COUNTER_BITS) - 1;
/// Mask selecting the physical-time bits of an NTP64 timestamp.
const PHYSICAL_MASK: u64 = !COUNTER_MASK;

/// Get the physical time in seconds from a timestamp.
#[must_use]
pub fn physical_time_secs(ts: &HybridTimestamp) -> u32 {
    (ts.0.get_time().as_u64() >> 32) as u32
}

/// Get the logical counter from a timestamp.
#[must_use]
pub fn logical_counter(ts: &HybridTimestamp) -> u32 {
    (ts.0.get_time().as_u64() & COUNTER_MASK) as u32
}

/// Hybrid Logical Clock implementation.
///
/// Implements the HLC algorithm with custom time/random sources for WASM compatibility.
/// Based on: https://github.com/atolab/uhlc-rs
pub(crate) struct LogicalClock {
    /// Unique ID for this HLC instance (randomly generated)
    id: u128,
    /// Last observed physical time in NTP64 format, quantized to
    /// [`PHYSICAL_MASK`] (the counter bits are always zero here so that
    /// `last_time` and emitted timestamps share one representation).
    last_time: u64,
    /// Logical counter (embedded in the low [`COUNTER_BITS`] of a timestamp)
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

    /// Set the clock to the logical tick immediately after `base_counter` at
    /// physical time `phys` (which must already be quantized to
    /// [`PHYSICAL_MASK`]). If the counter would overflow its reserved bits,
    /// carry into physical time so the clock keeps moving strictly forward
    /// instead of wrapping back to zero.
    fn set_after(&mut self, phys: u64, base_counter: u16) {
        self.last_time = phys;
        if let Some(next) = base_counter.checked_add(1) {
            self.counter = next;
        } else {
            self.last_time = phys.wrapping_add(1 << COUNTER_BITS);
            self.counter = 0;
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
        // Quantize physical time to the bits not reserved for the counter so
        // that `last_time` and emitted timestamps use one representation.
        let physical_time = ((secs << 32) | frac) & PHYSICAL_MASK;

        // HLC algorithm: time = max(physical, last_observed)
        if physical_time > self.last_time {
            self.last_time = physical_time;
            self.counter = 0;
        } else {
            // Clock didn't advance - advance the logical counter past the last
            // event (carrying into physical time if the counter overflows).
            self.set_after(self.last_time, self.counter);
        }

        // Embed the counter in the reserved low bits of the timestamp.
        let time_with_counter = NTP64(self.last_time | u64::from(self.counter));

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
        let remote_phys = remote_time & PHYSICAL_MASK;
        let remote_counter = (remote_time & COUNTER_MASK) as u16;

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

        // Full Hybrid Logical Clock update rule. Advance to the greatest
        // physical time among the local clock, the observed remote timestamp,
        // and the local wall clock. Whenever physical time does not strictly
        // increase, advance the counter past every event seen at that time so
        // the next locally issued timestamp sorts strictly after both the
        // previous local event and the remote event we just observed.
        //
        // The previous implementation only took `max(local, remote)` of the
        // raw value and reset the counter to zero on the greater case (never
        // reading the remote counter, never moving it on the equal case). That
        // let a later local event tie or precede an event it causally follows,
        // inverting the ordering an HLC exists to guarantee.
        let now_phys = local_ntp & PHYSICAL_MASK;
        let new_phys = self.last_time.max(remote_phys).max(now_phys);

        if new_phys == self.last_time && new_phys == remote_phys {
            // Same tick on both sides: outrank both the local and remote event.
            self.set_after(new_phys, self.counter.max(remote_counter));
        } else if new_phys == self.last_time {
            // Local clock already at the max tick: outrank the local event.
            self.set_after(new_phys, self.counter);
        } else if new_phys == remote_phys {
            // Remote tick is the max: outrank the remote event we observed.
            self.set_after(new_phys, remote_counter);
        } else {
            // Local wall clock is strictly ahead: a fresh physical tick.
            self.last_time = new_phys;
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

    /// Build a remote timestamp at a given physical tick and counter.
    fn remote_ts(phys: u64, counter: u16) -> HybridTimestamp {
        let raw = (phys & PHYSICAL_MASK) | u64::from(counter);
        HybridTimestamp::from(Timestamp::new(NTP64(raw), ID::from(DEFAULT_ID)))
    }

    #[test]
    fn test_logical_counter_reads_full_counter_width() {
        // Counters above 15 must round-trip; a prior `& 0xF` mask truncated
        // the 16-bit counter to 4 bits.
        let ts = remote_ts(1_u64 << 32, 0x00AB);
        assert_eq!(logical_counter(&ts), 0x00AB);
    }

    #[test]
    fn test_update_equal_physical_time_bumps_counter() {
        // A local and a remote event share the same physical tick, and the
        // remote counter is ahead of ours.
        let time = AtomicU64::new(1_000_000_000_000_000_000);
        let mut hlc = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));

        let _ = hlc.new_timestamp(|| time.load(Ordering::Relaxed));
        let local = hlc.new_timestamp(|| time.load(Ordering::Relaxed));

        let remote = remote_ts(local.get_time().as_u64(), 9);
        hlc.update(&remote, || time.load(Ordering::Relaxed))
            .unwrap();

        // The next local timestamp must sort strictly after the remote event.
        let next = hlc.new_timestamp(|| time.load(Ordering::Relaxed));
        assert!(
            next.get_time().as_u64() > remote.get_time().as_u64(),
            "next {next} must outrank observed remote {remote}",
        );
    }

    #[test]
    fn test_update_greater_physical_time_uses_remote_counter() {
        // Remote is one physical tick ahead with a non-zero counter; the local
        // wall clock stays behind, so ordering must fall to the counter.
        let time = AtomicU64::new(1_000_000_000_000_000_000);
        let mut hlc = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));
        let local = hlc.new_timestamp(|| time.load(Ordering::Relaxed));

        let remote_phys = (local.get_time().as_u64() & PHYSICAL_MASK) + (1 << COUNTER_BITS);
        let remote = remote_ts(remote_phys, 9);
        hlc.update(&remote, || time.load(Ordering::Relaxed))
            .unwrap();

        let next = hlc.new_timestamp(|| time.load(Ordering::Relaxed));
        assert!(next.get_time().as_u64() > remote.get_time().as_u64());
        // Still on the remote's physical tick: no spurious time jump.
        assert_eq!(next.get_time().as_u64() & PHYSICAL_MASK, remote_phys);
    }

    #[test]
    fn test_update_preserves_cross_node_causality() {
        // B issues many events on a stalled tick; A observes B's latest and
        // must issue a strictly-later timestamp despite its own low counter.
        let time = AtomicU64::new(1_000_000_000_000_000_000);
        let mut a = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));
        let mut b = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));

        let mut b_ts = b.new_timestamp(|| time.load(Ordering::Relaxed));
        for _ in 0..20 {
            b_ts = b.new_timestamp(|| time.load(Ordering::Relaxed));
        }

        let _ = a.new_timestamp(|| time.load(Ordering::Relaxed));
        a.update(&b_ts, || time.load(Ordering::Relaxed)).unwrap();
        let a_next = a.new_timestamp(|| time.load(Ordering::Relaxed));

        assert!(
            a_next.get_time().as_u64() > b_ts.get_time().as_u64(),
            "A's post-observe event {a_next} must follow B's observed event {b_ts}",
        );
    }

    #[test]
    fn test_counter_carry_preserves_monotonicity() {
        // A stalled physical clock issuing more events than the counter can
        // hold must still produce strictly increasing timestamps (carry into
        // physical time rather than wrapping the counter back to zero).
        let time = AtomicU64::new(1_000_000_000_000_000_000);
        let mut hlc = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));
        let mut prev = hlc.new_timestamp(|| time.load(Ordering::Relaxed));
        for _ in 0..70_000 {
            let next = hlc.new_timestamp(|| time.load(Ordering::Relaxed));
            assert!(
                next.get_time().as_u64() > prev.get_time().as_u64(),
                "timestamps must be strictly monotonic across counter overflow",
            );
            prev = next;
        }
    }

    #[test]
    fn test_update_rejects_far_future() {
        // Remote timestamps more than the drift tolerance ahead are rejected.
        let time = AtomicU64::new(1_000_000_000_000_000_000);
        let mut hlc = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));
        let now = hlc.new_timestamp(|| time.load(Ordering::Relaxed));

        let future = remote_ts(now.get_time().as_u64() + (10_u64 << 32), 0);
        assert!(hlc
            .update(&future, || time.load(Ordering::Relaxed))
            .is_err());
    }
}
