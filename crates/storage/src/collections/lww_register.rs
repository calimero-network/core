//! Last-Write-Wins Register - A CRDT for single values
//!
//! The LWW Register resolves conflicts by choosing the value with the latest timestamp.
//! When timestamps are equal, it uses node_id for deterministic tie-breaking.
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

use borsh::{BorshDeserialize, BorshSerialize};

use crate::env;
use crate::logical_clock::HybridTimestamp;

/// Last-Write-Wins Register - a CRDT for single values
///
/// Automatically resolves conflicts by timestamp, with node_id tie-breaking.
/// Safe to use in concurrent multi-node environments.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct LwwRegister<T> {
    /// The current value
    value: T,
    /// HLC timestamp of last write
    timestamp: HybridTimestamp,
    /// Node that performed the write (for tie-breaking)
    node_id: [u8; 32],
}

impl<T> LwwRegister<T> {
    /// Create a new LWW register with the given value
    ///
    /// Uses current HLC timestamp and executor ID.
    /// During merge mode, uses zero timestamp to ensure deterministic hashes.
    pub fn new(value: T) -> Self {
        // During merge mode, use deterministic zero timestamp to prevent
        // hash divergence from different nodes generating different timestamps.
        if env::in_merge_mode() {
            Self {
                value,
                timestamp: crate::logical_clock::HybridTimestamp::zero(),
                node_id: [0; 32],
            }
        } else {
            Self {
                value,
                timestamp: env::hlc_timestamp(),
                node_id: env::executor_id(),
            }
        }
    }

    /// Create a new LWW register with explicit timestamp and node_id
    ///
    /// Useful for testing or manual construction.
    pub fn new_with_metadata(value: T, timestamp: HybridTimestamp, node_id: [u8; 32]) -> Self {
        Self {
            value,
            timestamp,
            node_id,
        }
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

    /// Mutate the value in place, stamping a fresh HLC timestamp + node id when
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

    /// Set a new value (updates timestamp and node_id)
    ///
    /// During merge mode (inside a `#[app::migrate]` body) the stamp is zeroed
    /// for cross-node determinism, exactly like [`LwwRegister::new`] — a
    /// migrate-written value becomes the new baseline (a genuine post-migration
    /// write with a real timestamp then supersedes it). Outside merge mode it
    /// stamps the current HLC + executor id as normal.
    pub fn set(&mut self, value: T) {
        self.value = value;
        if env::in_merge_mode() {
            self.timestamp = HybridTimestamp::zero();
            self.node_id = [0; 32];
        } else {
            self.timestamp = env::hlc_timestamp();
            self.node_id = env::executor_id();
        }
    }

    /// Get the timestamp of the last write
    #[must_use]
    pub fn timestamp(&self) -> HybridTimestamp {
        self.timestamp
    }

    /// Get the node ID of the last write
    #[must_use]
    pub fn node_id(&self) -> [u8; 32] {
        self.node_id
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
    /// 1. If `other.timestamp > self.timestamp` → take other's value
    /// 2. If timestamps equal → use node_id for tie-breaking (higher wins)
    /// 3. If timestamps and node_id are equal → use serialized value bytes for
    ///    tie-breaking (handles merge-mode zero-stamps where both fields are
    ///    `[0;32]` / zero but values differ — without this, merge is
    ///    non-commutative and replicas permanently diverge)
    /// 4. Otherwise → keep current value
    ///
    /// This ensures deterministic, conflict-free merging across all nodes.
    pub fn merge(&mut self, other: &Self) {
        let should_update = Self::other_wins(
            other.timestamp,
            other.node_id,
            &other.value,
            self.timestamp,
            self.node_id,
            &self.value,
        );

        if should_update {
            self.value = other.value.clone();
            self.timestamp = other.timestamp;
            self.node_id = other.node_id;
        }
    }

    /// Check if this register would be updated by merging with `other`
    ///
    /// Useful for detecting conflicts before applying merge.
    #[must_use]
    pub fn would_update(&self, other: &Self) -> bool {
        Self::other_wins(
            other.timestamp,
            other.node_id,
            &other.value,
            self.timestamp,
            self.node_id,
            &self.value,
        )
    }

    /// Deterministic comparison: returns true when the (ts_b, id_b, val_b)
    /// tuple should win over (ts_a, id_a, val_a).
    fn other_wins(
        ts_b: HybridTimestamp,
        id_b: [u8; 32],
        val_b: &T,
        ts_a: HybridTimestamp,
        id_a: [u8; 32],
        val_a: &T,
    ) -> bool {
        if ts_b != ts_a {
            return ts_b > ts_a;
        }
        if id_b != id_a {
            return id_b > id_a;
        }
        // Both stamp fields are equal (including the merge-mode zero-zero case).
        // Fall back to lexicographic comparison of borsh-serialized bytes so
        // the merge is commutative even when values differ.
        // This branch is only reachable in the degenerate zero-stamp scenario
        // (merge mode with divergent values); the allocation cost is acceptable.
        let bytes_b = borsh::to_vec(val_b)
            .expect("BorshSerialize is guaranteed by the trait bound; serialization must not fail");
        let bytes_a = borsh::to_vec(val_a)
            .expect("BorshSerialize is guaranteed by the trait bound; serialization must not fail");
        bytes_b > bytes_a
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
/// in-place mutation and stamps a fresh HLC timestamp + node id on drop (only if
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
        // Same stamping rule as `LwwRegister::set` (incl. merge-mode zeroing for
        // cross-node determinism inside `#[app::migrate]`).
        if env::in_merge_mode() {
            self.reg.timestamp = HybridTimestamp::zero();
            self.reg.node_id = [0; 32];
        } else {
            self.reg.timestamp = env::hlc_timestamp();
            self.reg.node_id = env::executor_id();
        }
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

#[cfg(test)]
mod merge_mode_tests {
    use super::*;
    use crate::collections::{Root, UnorderedMap};
    use crate::env;
    use crate::logical_clock::HybridTimestamp;

    #[test]
    fn lww_new_zeroes_timestamp_and_node_id_in_merge_mode() {
        env::reset_for_testing();
        env::set_executor_id([7; 32]);

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
        assert_eq!(
            inside.node_id(),
            [0; 32],
            "LwwRegister::new inside merge mode must zero the node_id"
        );
    }

    #[test]
    fn lww_via_into_is_byte_identical_across_executors_in_merge_mode() {
        // Mirror the `total: count.into()` migrate path: two nodes with
        // different executor ids must serialise identical bytes.
        env::reset_for_testing();
        env::set_executor_id([1; 32]);
        let n1: LwwRegister<u64> = env::with_merge_mode(|| 6_u64.into());
        let b1 = borsh::to_vec(&n1).unwrap();

        env::reset_for_testing();
        env::set_executor_id([2; 32]);
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
        env::set_executor_id([9; 32]);

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
        env::set_executor_id([9; 32]);

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
        assert_eq!(total.node_id(), [0; 32]);
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
        assert_eq!(a.node_id(), [0; 32]);
        assert_eq!(b.node_id(), [0; 32]);

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
    fn lww_set_zeroes_timestamp_and_node_id_in_merge_mode() {
        env::reset_for_testing();
        env::set_executor_id([7; 32]);

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
        assert_eq!(
            reg.node_id(),
            [0; 32],
            "LwwRegister::set inside merge mode must zero the node_id"
        );
    }
}
