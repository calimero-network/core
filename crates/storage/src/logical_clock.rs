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

/// Derive a 16-byte HLC instance seed from a 32-byte executor id.
///
/// Must be collision-resistant: distinct executors need distinct seeds, or two
/// concurrently-minted `CharId`s collide and a character is silently lost during
/// RGA sync. Takes the first 16 bytes of `SHA-256(executor_id)` (`sha2` is
/// already a dep). The replaced code copied only the first 16 bytes, so keys
/// sharing a 16-byte prefix collided.
#[must_use]
pub fn hlc_seed_from_executor_id(executor_id: &[u8; 32]) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(executor_id);
    let mut seed = [0u8; 16];
    seed.copy_from_slice(&digest[..16]);
    seed
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
            // Carry one tick into physical time. Re-mask defensively so the
            // counter bits stay zero even if a future caller passes an
            // unquantized `phys` (the OR in `new_timestamp` relies on this).
            self.last_time = phys.wrapping_add(1 << COUNTER_BITS) & PHYSICAL_MASK;
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
        // `set_after` always leaves `last_time` quantized (counter bits zero)
        // and `counter` in range, so this OR never collides.
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

        // Drift protection: reject if >5s in future. Compare physical time
        // only — the remote counter bits are logical ordering, not clock
        // drift, and would otherwise inflate the comparison at the boundary.
        const DRIFT_TOLERANCE_SECS: u64 = 5;
        let drift_ntp = local_ntp + (DRIFT_TOLERANCE_SECS << 32);

        if remote_phys > drift_ntp {
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
    fn test_update_local_clock_ahead_of_remote_still_advances() {
        // Branch 2: local clock is already at the max tick and the remote is
        // behind. Observing it must still move the counter past the local
        // event so the next local timestamp is strictly greater.
        let time = AtomicU64::new(1_000_000_000_000_000_000);
        let mut hlc = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));
        let _ = hlc.new_timestamp(|| time.load(Ordering::Relaxed));
        let local = hlc.new_timestamp(|| time.load(Ordering::Relaxed));

        // Remote a full tick in the past.
        let remote_phys = (local.get_time().as_u64() & PHYSICAL_MASK) - (1 << COUNTER_BITS);
        let remote = remote_ts(remote_phys, 7);
        hlc.update(&remote, || time.load(Ordering::Relaxed))
            .unwrap();

        let next = hlc.new_timestamp(|| time.load(Ordering::Relaxed));
        assert!(next.get_time().as_u64() > local.get_time().as_u64());
        assert!(next.get_time().as_u64() > remote.get_time().as_u64());
    }

    #[test]
    fn test_update_wall_clock_ahead_resets_counter() {
        // Branch 4: the local wall clock has advanced strictly past both the
        // local HLC state and the remote timestamp, so the resulting tick is
        // the wall clock's and the counter resets to zero.
        let time = AtomicU64::new(1_000_000_000_000_000_000);
        let mut hlc = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));

        // Build up a non-zero counter on the starting tick.
        let _ = hlc.new_timestamp(|| time.load(Ordering::Relaxed));
        let start = hlc.new_timestamp(|| time.load(Ordering::Relaxed));

        // A remote event one tick behind the start.
        let remote = remote_ts(
            (start.get_time().as_u64() & PHYSICAL_MASK) - (1 << COUNTER_BITS),
            4,
        );

        // Advance the wall clock by two seconds (strictly ahead of both).
        let ahead = time.load(Ordering::Relaxed) + 2 * 1_000_000_000;
        time.store(ahead, Ordering::Relaxed);
        hlc.update(&remote, || time.load(Ordering::Relaxed))
            .unwrap();

        // `update` reset the counter to 0 on the fresh wall-clock tick, so the
        // next local timestamp is the first event of that tick (counter 1).
        let next = hlc.new_timestamp(|| time.load(Ordering::Relaxed));
        assert!(physical_time_secs(&next) > physical_time_secs(&start));
        assert_eq!(logical_counter(&next), 1);
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

    /// A naive XOR-fold (`seed[i % 16] ^= key[i]`) collapses any `[k; 32]` key to
    /// an all-zero seed, colliding every such executor's HLC id — which is why the
    /// seed uses a SHA-256 prefix, not a fold.
    #[test]
    fn test_naive_xor_fold_collapses_repeated_key_to_zero() {
        // The rejected XOR-fold alternative, reproduced verbatim.
        fn xor_fold_seed(executor_id: &[u8; 32]) -> [u8; 16] {
            let mut seed = [0u8; 16];
            for (i, byte) in executor_id.iter().enumerate() {
                seed[i % 16] ^= *byte;
            }
            seed
        }

        // `[k; 32]` collapses to all-zero for ANY byte k.
        for k in [1u8, 7, 42, 255] {
            assert_eq!(
                xor_fold_seed(&[k; 32]),
                [0u8; 16],
                "XOR-fold collapses [{k}; 32] to an all-zero seed (this is the bug)"
            );
        }

        // Two DISTINCT repeated keys therefore collide on the same seed.
        assert_eq!(
            xor_fold_seed(&[1u8; 32]),
            xor_fold_seed(&[2u8; 32]),
            "distinct repeated keys collide under the XOR-fold (this is the bug)"
        );
    }

    /// The SHA-256 seeding is collision-resistant: distinct executor ids — even
    /// the adversarial `[k; 32]` family that XOR-fold collapsed — yield DISTINCT
    /// seeds and DISTINCT HLC ids, so no `CharId` collision and no silent
    /// character loss during RGA sync.
    #[test]
    fn test_sha256_seeding_distinct_for_distinct_keys() {
        // Adversarial set: the repeated-byte keys the XOR-fold mapped to one
        // all-zero seed, plus shared-prefix keys, plus a structured key.
        let keys: [[u8; 32]; 6] = [
            [1u8; 32],
            [2u8; 32],
            [255u8; 32],
            {
                let mut k = [7u8; 32];
                k[16] = 1; // shares the low 16 bytes with the next
                k
            },
            {
                let mut k = [7u8; 32];
                k[16] = 2;
                k
            },
            [0u8; 32], // genuine all-zero input (zero→1 guard territory)
        ];

        // Every pair of distinct keys must map to distinct seeds.
        let seeds: Vec<[u8; 16]> = keys.iter().map(hlc_seed_from_executor_id).collect();
        for i in 0..seeds.len() {
            for j in (i + 1)..seeds.len() {
                assert_ne!(
                    seeds[i], seeds[j],
                    "distinct keys {:?} / {:?} must seed distinctly (collision-resistance)",
                    keys[i], keys[j]
                );
            }
        }

        // And the production seeding path mints distinct HLC ids for the
        // previously-colliding [1;32] vs [2;32] pair.
        let mut hlc_a =
            LogicalClock::new(|buf| buf.copy_from_slice(&hlc_seed_from_executor_id(&[1u8; 32])));
        let mut hlc_b =
            LogicalClock::new(|buf| buf.copy_from_slice(&hlc_seed_from_executor_id(&[2u8; 32])));
        let time = AtomicU64::new(1_000_000_000_000_000_000);
        let ts_a = hlc_a.new_timestamp(|| time.load(Ordering::Relaxed));
        let ts_b = hlc_b.new_timestamp(|| time.load(Ordering::Relaxed));
        assert_ne!(
            ts_a.get_id(),
            ts_b.get_id(),
            "[1;32] and [2;32] executors must mint distinct HLC ids (XOR-fold collapsed both)"
        );
    }

    /// The SHA-256 prefix never collapses a non-zero key to the all-zero seed
    /// (producing an all-zero 16-byte prefix would need an infeasible preimage),
    /// so the constructor's zero→1 guard only ever fires for genuine input that
    /// happens to hash to a zero prefix — which no real key does.
    #[test]
    fn test_sha256_seeding_repeated_key_is_non_zero() {
        // The exact inputs the XOR-fold zeroed must now seed to a non-zero id.
        for k in [1u8, 7, 42, 255] {
            assert_ne!(
                hlc_seed_from_executor_id(&[k; 32]),
                [0u8; 16],
                "SHA-256 seeding of [{k}; 32] must not be all-zero (XOR-fold's bug)"
            );
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
