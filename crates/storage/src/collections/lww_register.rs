//! Last-Write-Wins Register - A CRDT for single values
//!
//! The LWW Register resolves conflicts by choosing the value with the latest timestamp.
//! The HLC's random per-clock id separates writers; when two stamps are equal
//! (only the merge-mode zero), the value bytes break the tie.
//!
//! ## Use Cases
//!
//! - Document titles, usernames, settings
//! - Any field that should keep the "last write"
//! - Alternative to manual LWW logic in application code
//!
//! ## Example
//!
//! ```ignore
//! use calimero_storage::collections::LwwRegister;
//!
//! let mut title = LwwRegister::new("Draft".to_string());
//! title.set("Final".to_string());
//!
//! assert_eq!(title.get(), "Final");
//!
//! // Concurrent updates merge deterministically
//! let mut node1 = LwwRegister::new("Alice's version".to_string());
//! let node2 = LwwRegister::new("Bob's version".to_string());
//!
//! node1.merge(&node2); // Latest timestamp wins
//! ```

use core::any::type_name;
use core::cell::Cell;
use std::io::{Error, ErrorKind, Read, Result as IoResult, Write};

use borsh::{BorshDeserialize, BorshSerialize};

use crate::env;
use crate::logical_clock::{is_beyond_drift, HybridTimestamp};

/// Last-Write-Wins Register - a CRDT for single values
///
/// Automatically resolves conflicts by HLC timestamp. The HLC carries a random
/// per-clock id, so two writers never share a stamp outside merge mode, where
/// both are zero and the value bytes break the tie.
/// Safe to use in concurrent multi-node environments.
///
/// A register that is a map entry's whole value is stored without its stamp:
/// see [`entry_stamp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LwwRegister<T> {
    /// The current value
    value: T,
    /// HLC timestamp of last write. Its random id already separates writers, so
    /// the register does not also store the writing device (32 bytes that used to
    /// break a tie two distinct clocks cannot produce).
    timestamp: HybridTimestamp,
}

impl<T> LwwRegister<T> {
    /// Create a new LWW register with the given value
    ///
    /// Uses the current HLC timestamp.
    /// During merge mode, uses zero timestamp to ensure deterministic hashes.
    pub fn new(value: T) -> Self {
        // During merge mode, use deterministic zero timestamp to prevent
        // hash divergence from different nodes generating different timestamps.
        if env::in_merge_mode() {
            Self {
                value,
                timestamp: crate::logical_clock::HybridTimestamp::zero(),
            }
        } else {
            Self {
                value,
                timestamp: env::hlc_timestamp(),
            }
        }
    }

    /// Create a new LWW register with an explicit timestamp.
    ///
    /// Useful for testing or manual construction.
    pub fn new_with_metadata(value: T, timestamp: HybridTimestamp) -> Self {
        Self { value, timestamp }
    }

    /// Get the current value
    #[must_use]
    pub fn get(&self) -> &T {
        &self.value
    }

    /// Get a mutable reference to the value.
    ///
    /// **WARNING:** direct mutation through this reference bypasses the
    /// timestamp update, so the write keeps a stale HLC stamp and last-write-wins
    /// merge will not pick it up — replicas silently diverge. Prefer
    /// [`value_mut`](LwwRegister::value_mut) (mutate in place, stamped on drop) or
    /// [`set`](LwwRegister::set) (replace wholesale).
    #[deprecated(
        note = "bypasses the HLC stamp and silently breaks convergence; use `value_mut()` \
                (in-place, stamped on drop) or `set()`"
    )]
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }

    /// Mutate the value in place, stamping a fresh HLC timestamp when
    /// the returned guard is dropped — the safe equivalent of mutating a plain
    /// field, without the [`get_mut`](LwwRegister::get_mut) footgun.
    ///
    /// The guard derefs to `&mut T`, so you edit the value with ordinary
    /// semantics; the stamp is applied once, on drop, and only if the value was
    /// actually mutated (a guard used for reads only leaves the clock untouched).
    /// In merge mode the stamp is zeroed for cross-node determinism, exactly like
    /// [`set`](LwwRegister::set).
    ///
    /// ```ignore
    /// // tweak one field of a struct value without clone+set:
    /// self.profile.value_mut().bio = "hi".to_owned();
    /// // multiple edits collapse to a single stamp at end of scope:
    /// { let mut items = self.list.value_mut(); items.push(x); items.retain(|i| i.ok); }
    /// ```
    pub fn value_mut(&mut self) -> LwwGuard<'_, T> {
        LwwGuard {
            reg: self,
            dirty: false,
        }
    }

    /// Set a new value (updates the timestamp)
    ///
    /// During merge mode (inside a `#[app::migrate]` body) the stamp is zeroed
    /// for cross-node determinism, exactly like [`LwwRegister::new`] — a
    /// migrate-written value becomes the new baseline (a genuine post-migration
    /// write with a real timestamp then supersedes it). Outside merge mode it
    /// stamps the current HLC + executor id as normal.
    pub fn set(&mut self, value: T) {
        self.value = value;
        self.stamp();
    }

    /// Stamps a write of this register: zero in merge mode, for cross-node
    /// determinism; otherwise the current HLC, or, when that is not later than
    /// the stamp being replaced, the tick after it.
    ///
    /// A write follows the value it replaces, so it must win against it. The
    /// HLC alone does not ensure that: it starts afresh with every execution,
    /// so its reading is the wall clock's, and a clock behind the stored stamp
    /// (an NTP step back, or a value written by a peer whose clock is ahead)
    /// stamped the new value older than the old one. This replica then held the
    /// new value while every peer kept the old, and they never converged.
    fn stamp(&mut self) {
        self.timestamp = if env::in_merge_mode() {
            HybridTimestamp::zero()
        } else {
            env::hlc_timestamp().after(self.timestamp)
        };
    }

    /// Get the timestamp of the last write
    #[must_use]
    pub fn timestamp(&self) -> HybridTimestamp {
        self.timestamp
    }

    /// Consume the register and return the inner value
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T: Clone + borsh::BorshSerialize> LwwRegister<T> {
    /// Merge with another register (CRDT merge operation)
    ///
    /// # Merge Rules
    ///
    /// 0. A timestamp further ahead of the local clock than the drift
    ///    tolerance loses to one within it, on either side
    /// 1. If `other.timestamp > self.timestamp` → take other's value. The HLC
    ///    orders by time, then by its random per-clock id, so two writers only
    ///    tie when both stamps are the merge-mode zero.
    /// 2. If timestamps are equal → use serialized value bytes for tie-breaking
    ///    (without this, merge-mode zero stamps over different values would be
    ///    non-commutative and replicas would permanently diverge)
    /// 3. Otherwise → keep current value
    ///
    /// This ensures deterministic, conflict-free merging across all nodes.
    pub fn merge(&mut self, other: &Self) {
        let should_update =
            Self::other_wins(other.timestamp, &other.value, self.timestamp, &self.value);

        if should_update {
            self.value = other.value.clone();
            self.timestamp = other.timestamp;
        }
    }

    /// Check if this register would be updated by merging with `other`
    ///
    /// Useful for detecting conflicts before applying merge.
    #[must_use]
    pub fn would_update(&self, other: &Self) -> bool {
        Self::other_wins(other.timestamp, &other.value, self.timestamp, &self.value)
    }

    /// Deterministic comparison: returns true when the (ts_b, val_b) pair
    /// should win over (ts_a, val_a); a stamp beyond the drift bound loses to
    /// one within it.
    fn other_wins(ts_b: HybridTimestamp, val_b: &T, ts_a: HybridTimestamp, val_a: &T) -> bool {
        let now = env::time_now();
        let (b_far, a_far) = (is_beyond_drift(&ts_b, now), is_beyond_drift(&ts_a, now));
        if b_far != a_far {
            return a_far;
        }
        if ts_b != ts_a {
            return ts_b > ts_a;
        }
        // The stamps are equal, which two distinct clocks cannot produce: this is
        // the merge-mode zero-zero case, or two map entries' registers, which read
        // their stamps from rows written in the same nanosecond.
        // Fall back to lexicographic comparison of borsh-serialized bytes so
        // the merge is commutative even when values differ.
        // This branch is only reachable in those degenerate scenarios; the
        // allocation cost is acceptable.
        let bytes_b = borsh::to_vec(val_b)
            .expect("BorshSerialize is guaranteed by the trait bound; serialization must not fail");
        let bytes_a = borsh::to_vec(val_a)
            .expect("BorshSerialize is guaranteed by the trait bound; serialization must not fail");
        bytes_b > bytes_a
    }
}

/// `value ‖ timestamp`, or just `value` for a register that is a map entry's
/// whole value, whose stamp is the entry's `updated_at` (see [`entry_stamp`]).
impl<T: BorshSerialize> BorshSerialize for LwwRegister<T> {
    fn serialize<W: Write>(&self, writer: &mut W) -> IoResult<()> {
        let elided = entry_stamp::claim::<Self>();
        self.value.serialize(writer)?;
        if elided {
            return Ok(());
        }
        self.timestamp.serialize(writer)
    }
}

impl<T: BorshDeserialize> BorshDeserialize for LwwRegister<T> {
    fn deserialize_reader<R: Read>(reader: &mut R) -> IoResult<Self> {
        let elided = entry_stamp::claim::<Self>();
        let value = T::deserialize_reader(reader)?;
        let timestamp = if elided {
            entry_stamp::stored()?
        } else {
            HybridTimestamp::deserialize_reader(reader)?
        };
        Ok(Self { value, timestamp })
    }
}

/// A register that is the whole value of an [`UnorderedMap`] or [`SortedMap`]
/// entry does not store its stamp: the entry's `updated_at` is its stamp.
///
/// Storing both was 16 bytes per entry spent on a stamp nothing reads. Such an
/// entry is one entity holding one register, and every path that resolves two
/// versions of it (a local write, a remote apply, a snapshot, a HashComparison
/// leaf) picks between whole entries by `updated_at`, the way it picks between
/// entries of any value type. The register's own HLC never took part; it was
/// not even equal to `updated_at`, which is a separate wall-clock reading.
///
/// So the register is written without it, and read back with
/// `HybridTimestamp::from_unix_nanos(updated_at)`. Where registers ARE compared
/// (fields of a state struct or of one stored value, merged field by field),
/// they are not an entry's whole value and keep their stamps. So do those
/// nested inside the value; a set's, whose entries are addressed by their own
/// bytes; and a `Vector`'s, which `Vector::merge` pairs across two vectors by
/// position and decides by the register's own stamp.
///
/// The collection names its value type ([`Collection::stamp_values_of`]); an entry of
/// that map passes it to the register for the duration of the entry's value
/// ([`within_entry`]); the register at the very start of the value, if it is of
/// exactly that type, claims it ([`claim`]); and a load names the stamp the
/// row stores ([`with_stored`]).
///
/// [`UnorderedMap`]: super::UnorderedMap
/// [`SortedMap`]: super::SortedMap
/// [`Collection::stamp_values_of`]: super::Collection
pub(crate) mod entry_stamp {
    use super::{type_name, Cell, Error, ErrorKind, HybridTimestamp, IoResult};

    thread_local! {
        /// The value type of the collection whose entry is being written or
        /// read, while its call into the store runs.
        static COLLECTION: Cell<Option<&'static str>> = const { Cell::new(None) };
        /// The value type of the entry being encoded, until a register of
        /// exactly that type claims it.
        static VALUE: Cell<Option<&'static str>> = const { Cell::new(None) };
        /// The `updated_at` of the entity being decoded.
        static STORED: Cell<Option<u64>> = const { Cell::new(None) };
    }

    /// Runs `f`, a collection's call into the store, with its entries' value
    /// type named.
    pub(crate) fn within_collection<R>(value: Option<&'static str>, f: impl FnOnce() -> R) -> R {
        let previous = COLLECTION.replace(value);
        let out = f();
        COLLECTION.set(previous);
        out
    }

    /// Runs `f`, the encoding of one entry's value, offering the register that
    /// starts it its stamp.
    pub(crate) fn within_entry<R>(f: impl FnOnce() -> R) -> R {
        let previous = VALUE.replace(COLLECTION.get());
        let out = f();
        VALUE.set(previous);
        out
    }

    /// Whether the register about to be encoded, of type `R`, is the entry's
    /// whole value and so goes without its stamp.
    ///
    /// Types are told apart by name, which needs no `'static` bound of the
    /// value. Only the value type itself can claim: a register nested in the
    /// value is a different type, and the offer is withdrawn by the claim
    /// anyway, before anything inside the claiming register is encoded.
    pub(super) fn claim<R: ?Sized>() -> bool {
        VALUE.with(|value| {
            let claimed = value.get() == Some(type_name::<R>());
            if claimed {
                value.set(None);
            }
            claimed
        })
    }

    /// Runs `f`, the decoding of an entity whose row is stamped `updated_at`.
    pub(crate) fn with_stored<R>(updated_at: Option<u64>, f: impl FnOnce() -> R) -> R {
        let previous = STORED.replace(updated_at);
        let out = f();
        STORED.set(previous);
        out
    }

    /// The stamp of the entry being decoded.
    pub(super) fn stored() -> IoResult<HybridTimestamp> {
        STORED
            .get()
            .map(HybridTimestamp::from_unix_nanos)
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidData,
                    "a map entry's register is stamped by its row, and none was read",
                )
            })
    }
}

impl<T: Default> Default for LwwRegister<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

// Deref for convenient read access without calling .get()
impl<T> std::ops::Deref for LwwRegister<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

/// RAII guard returned by [`LwwRegister::value_mut`]. Derefs to `&mut T` for
/// in-place mutation and stamps a fresh HLC timestamp on drop (only if
/// the value was actually mutated). This makes "mutate like a plain field" sound
/// for a CRDT register — the stamp can't be forgotten the way it can with
/// [`LwwRegister::get_mut`].
#[must_use = "the value is stamped when the guard is dropped; bind it (or mutate through it) \
              rather than discarding it"]
pub struct LwwGuard<'a, T> {
    reg: &'a mut LwwRegister<T>,
    dirty: bool,
}

impl<T> core::ops::Deref for LwwGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.reg.value
    }
}

impl<T> core::ops::DerefMut for LwwGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Any mutable access counts as a write; stamp on drop.
        self.dirty = true;
        &mut self.reg.value
    }
}

impl<T> Drop for LwwGuard<'_, T> {
    fn drop(&mut self) {
        if !self.dirty {
            return;
        }
        self.reg.stamp();
    }
}

// AsRef for automatic conversion to reference
impl<T> AsRef<T> for LwwRegister<T> {
    fn as_ref(&self) -> &T {
        &self.value
    }
}

// Borrow for compatibility with HashMap, BTreeMap, etc.
impl<T> std::borrow::Borrow<T> for LwwRegister<T> {
    fn borrow(&self) -> &T {
        &self.value
    }
}

// From inner value to create LwwRegister
impl<T> From<T> for LwwRegister<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

// Display for easy debugging
impl<T: std::fmt::Display> std::fmt::Display for LwwRegister<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

/// A register indexes as whatever it holds.
impl<T: calimero_sdk::search::SearchText> calimero_sdk::search::SearchText for LwwRegister<T> {
    fn search_text(&self) -> Option<String> {
        self.get().search_text()
    }
}

/// A register indexes as whatever it holds.
impl<T: calimero_sdk::search::SearchNumber> calimero_sdk::search::SearchNumber for LwwRegister<T> {
    fn search_number(&self) -> Option<u64> {
        self.get().search_number()
    }
}

#[cfg(test)]
mod merge_mode_tests {
    use super::*;
    use crate::collections::{Root, UnorderedMap};
    use crate::env;
    use crate::logical_clock::HybridTimestamp;

    #[test]
    fn lww_new_zeroes_timestamp_in_merge_mode() {
        env::reset_for_testing();
        env::set_device_id([7; 32]);

        // Outside merge mode: real, node-local stamp.
        let outside = LwwRegister::new(5_u64);
        assert_ne!(
            outside.timestamp(),
            HybridTimestamp::zero(),
            "outside merge mode should use a real HLC"
        );

        // Inside merge mode: deterministic zero stamp.
        let inside = env::with_merge_mode(|| LwwRegister::new(5_u64));
        assert_eq!(
            inside.timestamp(),
            HybridTimestamp::zero(),
            "LwwRegister::new inside merge mode must zero the timestamp"
        );
    }

    #[test]
    fn lww_via_into_is_byte_identical_across_executors_in_merge_mode() {
        // Mirror the `total: count.into()` migrate path: two nodes with
        // different executor ids must serialise identical bytes.
        env::reset_for_testing();
        env::set_device_id([1; 32]);
        let n1: LwwRegister<u64> = env::with_merge_mode(|| 6_u64.into());
        let b1 = borsh::to_vec(&n1).unwrap();

        env::reset_for_testing();
        env::set_device_id([2; 32]);
        let n2: LwwRegister<u64> = env::with_merge_mode(|| 6_u64.into());
        let b2 = borsh::to_vec(&n2).unwrap();

        assert_eq!(
            hex::encode(&b1),
            hex::encode(&b2),
            "LwwRegister built via .into() in merge mode must be byte-identical across executors"
        );
    }

    /// A nested `with_merge_mode` (e.g. the CRDT merge dispatch firing while
    /// a `#[app::migrate]` body holds the outer scope) must NOT clear merge
    /// mode for the remainder of the outer scope. Regression guard for the
    /// `invariant-reshuffle` divergence.
    #[test]
    fn nested_with_merge_mode_must_not_disable_outer_scope() {
        env::reset_for_testing();
        env::set_device_id([9; 32]);

        let (before, after) = env::with_merge_mode(|| {
            let before = LwwRegister::new(1_u64);
            // nested scope, mirrors a merge/apply opening its own merge mode
            let _nested = env::with_merge_mode(|| LwwRegister::new(99_u64));
            assert!(
                env::in_merge_mode(),
                "outer merge mode cleared by nested with_merge_mode"
            );
            let after = LwwRegister::new(2_u64); // like trailing `total: count.into()`
            (before, after)
        });

        let zero = HybridTimestamp::zero();
        assert_eq!(before.timestamp(), zero, "value before nested call");
        assert_eq!(
            after.timestamp(),
            zero,
            "value after nested call must still be zeroed"
        );
    }

    /// Faithful repro of the scenario-13 migrate body order: populate an
    /// `UnorderedMap` (whose insert path opens its own merge scope), THEN
    /// build a trailing `LwwRegister` (`total: total.into()`). Before the
    /// re-entrancy fix the map inserts cleared merge mode and the trailing
    /// register baked a node-local HLC into the migrated root.
    #[test]
    fn trailing_lww_after_map_inserts_stays_zeroed_in_merge_mode() {
        env::reset_for_testing();
        env::set_device_id([9; 32]);

        let total: LwwRegister<u64> = env::with_merge_mode(|| {
            let mut map = Root::new(UnorderedMap::<String, LwwRegister<u64>>::new);
            map.insert("a".to_string(), 1_u64.into()).unwrap();
            map.insert("b".to_string(), 2_u64.into()).unwrap();
            assert!(
                env::in_merge_mode(),
                "merge mode flipped OFF during/after map inserts"
            );
            3_u64.into()
        });

        assert_eq!(total.timestamp(), HybridTimestamp::zero());
    }

    /// Two merge-mode registers with different values must converge to the same
    /// winner regardless of which side calls merge first (commutativity check).
    #[test]
    fn merge_mode_equal_stamps_different_values_converge() {
        env::reset_for_testing();

        // Simulate two nodes that ran the same migrate body but ended up with
        // different values (e.g. due to a bug upstream).  Both have zero stamps.
        let a: LwwRegister<u64> = env::with_merge_mode(|| LwwRegister::new(1_u64));
        let b: LwwRegister<u64> = env::with_merge_mode(|| LwwRegister::new(2_u64));

        assert_eq!(a.timestamp(), HybridTimestamp::zero());
        assert_eq!(b.timestamp(), HybridTimestamp::zero());

        // merge(A, B) and merge(B, A) must produce the same winner.
        let mut ab = a.clone();
        ab.merge(&b);

        let mut ba = b.clone();
        ba.merge(&a);

        assert_eq!(
            ab.get(),
            ba.get(),
            "merge(A,B) and merge(B,A) must converge to the same value"
        );
    }

    /// `set()` on a carried-over register inside a migrate body must zero its
    /// stamp exactly like `new()` does. Otherwise the value bakes this node's
    /// wall-clock HLC + executor id into the migrated root and diverges across
    /// nodes — the latent `.set()` footgun the SDK docs wrongly claimed was
    /// already covered by merge mode.
    #[test]
    fn lww_set_zeroes_timestamp_in_merge_mode() {
        env::reset_for_testing();
        env::set_device_id([7; 32]);

        // Outside merge mode: a real, node-local stamp.
        let mut reg = LwwRegister::new(1_u64);
        assert_ne!(reg.timestamp(), HybridTimestamp::zero());

        // Inside merge mode: `.set()` must zero the stamp.
        env::with_merge_mode(|| reg.set(5_u64));

        assert_eq!(reg.get(), &5_u64);
        assert_eq!(
            reg.timestamp(),
            HybridTimestamp::zero(),
            "LwwRegister::set inside merge mode must zero the timestamp"
        );
    }
}
