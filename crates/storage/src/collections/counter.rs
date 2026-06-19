//! Counter collection - CRDT-compatible counter supporting both G-Counter and PN-Counter modes.
//!
//! This module provides a unified counter implementation that can operate as either:
//! - **G-Counter** (Grow-Only Counter): Increment-only, lighter weight (default)
//! - **PN-Counter** (Positive-Negative Counter): Supports both increment and decrement
//!
//! The mode is selected at compile-time using const generics for zero runtime overhead.

use borsh::io::{ErrorKind, Read, Result as BorshResult, Write};
use borsh::{BorshDeserialize, BorshSerialize};

use super::{CrdtType, StorageAdaptor, UnorderedMap};
use crate::collections::error::StoreError;
use crate::interface::StorageError;
use crate::store::MainStorage;

/// A CRDT-compatible counter with configurable increment/decrement support.
///
/// # Type Parameters
/// - `ALLOW_DECREMENT`: When `false` (default), acts as G-Counter (increment-only).
///                      When `true`, acts as PN-Counter (supports decrement).
/// - `S`: Storage adaptor (defaults to `MainStorage`)
///
/// # G-Counter Mode (ALLOW_DECREMENT = false)
///
/// Lightweight increment-only counter. Returns `u64` values.
///
/// ```ignore
/// use calimero_storage::collections::Counter;
///
/// #[app::state]
/// struct MyApp {
///     visit_count: Counter,  // Same as Counter<false>
/// }
///
/// impl MyApp {
///     pub fn increment_visitor(&mut self) {
///         self.visit_count.increment()?;
///     }
///     
///     pub fn total_visits(&self) -> u64 {
///         self.visit_count.value_unsigned()?
///     }
/// }
/// ```
///
/// # PN-Counter Mode (ALLOW_DECREMENT = true)
///
/// Supports both increment and decrement. Returns `i64` values (can be negative).
///
/// ```ignore
/// use calimero_storage::collections::PNCounter;
///
/// #[app::state]
/// struct Inventory {
///     stock: PNCounter,  // Same as Counter<true>
/// }
///
/// impl Inventory {
///     pub fn add_stock(&mut self, amount: u64) {
///         for _ in 0..amount {
///             self.stock.increment()?;
///         }
///     }
///     
///     pub fn remove_stock(&mut self, amount: u64) {
///         for _ in 0..amount {
///             self.stock.decrement()?;
///         }
///     }
///     
///     pub fn current_stock(&self) -> i64 {
///         self.stock.value_signed()?
///     }
/// }
/// ```
#[derive(Debug)]
pub struct Counter<const ALLOW_DECREMENT: bool = false, S: StorageAdaptor = MainStorage> {
    /// Maps executor_id (hex string) -> positive increments
    pub(crate) positive: UnorderedMap<String, u64, S>,

    /// Maps executor_id (hex string) -> negative decrements
    /// Only used when ALLOW_DECREMENT = true
    pub(crate) negative: UnorderedMap<String, u64, S>,
}

// Custom serialization: conditionally serialize fields based on ALLOW_DECREMENT
//
// Serialization format:
// - GCounter (ALLOW_DECREMENT = false): [positive_map]
// - PNCounter (ALLOW_DECREMENT = true): [positive_map][negative_map]
//
// This ensures:
// 1. GCounter uses less storage (no negative field serialized)
// 2. Type safety: Cannot deserialize PNCounter with negative counts as GCounter
impl<const ALLOW_DECREMENT: bool, S: StorageAdaptor> BorshSerialize
    for Counter<ALLOW_DECREMENT, S>
{
    fn serialize<W: Write>(&self, writer: &mut W) -> BorshResult<()> {
        // Always serialize positive counts
        self.positive.serialize(writer)?;

        if ALLOW_DECREMENT {
            // Only serialize negative counts for PNCounter
            self.negative.serialize(writer)?;
        }

        Ok(())
    }
}

// Custom deserialization: handle counter type compatibility
//
// Deserialization behavior:
//
// 1. GCounter ← GCounter: ✅ Works perfectly
// 2. GCounter ← PNCounter: ❌ ERROR - Borsh detects leftover bytes, prevents data loss
// 3. PNCounter ← GCounter: ✅ Safe upgrade (initializes empty negative map)
// 4. PNCounter ← PNCounter: ✅ Works perfectly
//
// Type safety mechanism:
// When a PNCounter is serialized, it writes [positive_map][negative_map].
// When deserializing as GCounter, only [positive_map] is read.
// Borsh's strict parsing detects the leftover [negative_map] bytes and fails with
// "Not all bytes read" or "Unexpected length of input", preventing silent data loss.
//
// This provides runtime type safety without explicit validation code.
impl<const ALLOW_DECREMENT: bool, S: StorageAdaptor> BorshDeserialize
    for Counter<ALLOW_DECREMENT, S>
{
    fn deserialize_reader<R: Read>(reader: &mut R) -> BorshResult<Self> {
        // Always deserialize positive counts
        let positive = UnorderedMap::deserialize_reader(reader)?;

        let negative = if ALLOW_DECREMENT {
            // PNCounter: Try to deserialize negative counts
            // This handles both:
            // 1. Deserializing a PNCounter (has negative field)
            // 2. Upgrading from GCounter to PNCounter (no negative field - safe upgrade)
            match UnorderedMap::<String, u64, S>::deserialize_reader(reader) {
                Ok(neg_map) => neg_map,
                Err(e) => {
                    // Only treat "no more data" errors as "no negative field" (safe GCounter upgrade)
                    // Propagate all other errors (corruption, I/O errors, etc.)
                    match e.kind() {
                        ErrorKind::UnexpectedEof => {
                            // Stream ended - no negative field present
                            // Use new_internal() because PNCounter needs a real negative map
                            UnorderedMap::new_internal()
                        }
                        ErrorKind::InvalidData if e.to_string().contains("Unexpected length") => {
                            // from_slice detected insufficient bytes - no negative field present
                            // This is a GCounter being upgraded to PNCounter (safe operation)
                            UnorderedMap::new_internal()
                        }
                        _ => {
                            // Data corruption or other error - propagate it
                            return Err(e);
                        }
                    }
                }
            }
        } else {
            // GCounter: Don't try to deserialize negative map
            // If PNCounter data is deserialized as GCounter, Borsh will fail with
            // "Not all bytes read" or "Unexpected length of input", preventing silent data loss.
            // This is the desired behavior - users must use the correct type.
            //
            // CRITICAL: Use new_detached() instead of new_internal() to prevent creating
            // a random child of ROOT_ID during deserialization. This was causing root hash
            // divergence because each deserialization would create a new random child ID.
            // The negative map for GCounter is never used, so it doesn't need to be
            // registered with the storage system.
            UnorderedMap::new_detached()
        };

        Ok(Counter { positive, negative })
    }
}

// Type aliases for convenience
/// G-Counter: Increment-only counter (lighter weight)
pub type GCounter<S = MainStorage> = Counter<false, S>;

/// PN-Counter: Supports increment and decrement
pub type PNCounter<S = MainStorage> = Counter<true, S>;

impl<const ALLOW_DECREMENT: bool> Counter<ALLOW_DECREMENT, MainStorage> {
    /// Creates a new counter with a random ID.
    ///
    /// Use this for nested collections stored as values in other maps.
    /// Merge happens by the parent map's key, so the nested collection's ID
    /// doesn't affect sync semantics.
    ///
    /// For top-level state fields, use `new_with_field_name` instead.
    #[must_use]
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Creates a new counter with a deterministic ID.
    ///
    /// The `field_name` is used to generate a deterministic collection ID,
    /// ensuring the same code produces the same ID across all nodes.
    ///
    /// Use this for top-level state fields (the `#[app::state]` macro does this
    /// automatically).
    ///
    /// # Example
    /// ```ignore
    /// let visits = Counter::new_with_field_name("visit_count");
    /// ```
    #[must_use]
    pub fn new_with_field_name(field_name: &str) -> Self {
        Self::new_with_field_name_internal(None, field_name)
    }
}

/// Re-key the counter's internal map(s) relative to its storage parent so two
/// nodes that independently first-create the same nested counter derive the
/// same internal-map id and their per-executor slots converge. See
/// [`super::rekey`].
impl<const ALLOW_DECREMENT: bool, S: StorageAdaptor> super::rekey::RekeyTarget
    for Counter<ALLOW_DECREMENT, S>
{
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        // The internal slot maps are plain `UnorderedMap`s of executor-id ->
        // count, and MUST be tagged as such — not with the counter's own
        // `GCounter`/`PnCounter` type. Each per-executor slot has a single
        // writer (its executor), so the slots converge by ordinary structural
        // sync. Tagging a slot map itself as a counter routes it through
        // `merge_by_crdt_type` -> `merge_pn_counter`, which deserializes the
        // single-map blob as a whole `Counter`, trips the missing-negative-map
        // upgrade fallback, and mints a stray random ROOT child on every merge
        // — leaving the receiver with orphan entities the writer never had and
        // diverging the root hash.
        let map_crdt_type = CrdtType::unordered_map(
            std::any::type_name::<String>(),
            std::any::type_name::<u64>(),
        );
        self.positive.reassign_deterministic_id_under(
            parent_id,
            "__counter_positive",
            map_crdt_type.clone(),
        );
        // GCounter's negative map is detached and never persisted — skip it.
        if ALLOW_DECREMENT {
            self.negative.reassign_deterministic_id_under(
                parent_id,
                "__counter_negative",
                map_crdt_type,
            );
        }
    }
}

impl<const ALLOW_DECREMENT: bool, S: StorageAdaptor> Counter<ALLOW_DECREMENT, S> {
    /// Creates a new counter (internal) - must use same visibility as UnorderedMap
    pub(super) fn new_internal() -> Self {
        super::rekey::register_rekey::<Self>();
        Self {
            positive: UnorderedMap::new_internal(),
            // GCounter (ALLOW_DECREMENT = false): negative map is never used,
            // so use detached to avoid adding a useless child to ROOT_ID.
            // PNCounter (ALLOW_DECREMENT = true): negative map is actually used,
            // so it needs to be properly registered with storage.
            negative: if ALLOW_DECREMENT {
                UnorderedMap::new_internal()
            } else {
                UnorderedMap::new_detached()
            },
        }
    }

    /// Creates a new counter with deterministic IDs (internal)
    pub(super) fn new_with_field_name_internal(
        parent_id: Option<crate::address::Id>,
        field_name: &str,
    ) -> Self {
        // Register the re-key thunk here too (not only in `new_internal`), so a
        // counter first constructed via the deterministic-id path still teaches
        // the registry about its type — keeps registration independent of which
        // constructor an app happens to hit first.
        super::rekey::register_rekey::<Self>();
        // For Counter, we need to create deterministic IDs for both positive and negative maps
        // Use a reserved internal prefix to prevent collisions with user-created collections.
        // The prefix "__counter_internal_" is reserved for Counter's internal maps and ensures
        // that a Counter with field name "X" won't collide with a user-created collection
        // named "X_positive" or "X_negative".
        //
        // Both internal maps are tagged as plain `UnorderedMap`s (the default in
        // `new_with_field_name_internal`), NOT with the counter's own
        // GCounter/PnCounter type — see `rekey_relative_to` for why tagging a slot
        // map as a counter mints orphan ROOT children during merge.
        Self {
            positive: UnorderedMap::new_with_field_name_internal(
                parent_id,
                &format!("__counter_internal_{field_name}_positive"),
            ),
            // GCounter: negative map is never used, so use detached
            // PNCounter: negative map is used, so register it properly
            negative: if ALLOW_DECREMENT {
                UnorderedMap::new_with_field_name_internal(
                    parent_id,
                    &format!("__counter_internal_{field_name}_negative"),
                )
            } else {
                UnorderedMap::new_detached()
            },
        }
    }

    /// Reassigns the counter's ID to a deterministic ID based on field name.
    ///
    /// This is called by the `#[app::state]` macro after `init()` returns to ensure
    /// all top-level collections have deterministic IDs regardless of how they were
    /// created in `init()`.
    ///
    /// This method also migrates all internal map entries to use the new parent IDs,
    /// ensuring that increments during `init()` remain accessible.
    ///
    /// # Arguments
    /// * `field_name` - The name of the struct field containing this counter
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        // Positive map: always needs deterministic ID and entry migration.
        // `reassign_deterministic_id` tags it with the natural `UnorderedMap`
        // CRDT type, which is what we want — the slot map must NOT carry the
        // counter's own GCounter/PnCounter type (see `rekey_relative_to`).
        self.positive
            .reassign_deterministic_id(&format!("__counter_internal_{field_name}_positive"));

        // Negative map: only for PNCounter (ALLOW_DECREMENT = true)
        // GCounter's negative map is detached and never used, so skip it
        if ALLOW_DECREMENT {
            self.negative
                .reassign_deterministic_id(&format!("__counter_internal_{field_name}_negative"));
        }
    }

    /// Increment the counter for the current executor
    ///
    /// # Panics
    /// Panics if called inside a state migration (`#[app::migrate]`, i.e. storage
    /// merge mode): `increment` stamps the running node's executor id, which differs
    /// per node and would diverge the network. Carry the counter across unchanged or
    /// use [`increment_for`](Self::increment_for) with a deterministic executor id.
    ///
    /// # Errors
    /// Returns error if storage operation fails
    #[expect(
        clippy::panic,
        reason = "non-deterministic during migrate (per-node executor id); a loud panic \
                  is the intended, unmissable guard against a silent network divergence"
    )]
    pub fn increment(&mut self) -> Result<(), StoreError> {
        if crate::env::in_merge_mode() {
            panic!(
                "Counter::increment() is non-deterministic during a state migration: it \
                 stamps this node's identity, which differs per node and diverges the \
                 network. Carry the counter across unchanged (`field: old.field`) or \
                 replay with `increment_for(executor_id, …)`."
            );
        }
        let executor_id = crate::env::executor_id();
        self.increment_for(&executor_id)
    }

    /// Increment the counter for a specific executor
    ///
    /// # Errors
    /// Returns error if storage operation fails or if increment would overflow u64::MAX
    pub fn increment_for(&mut self, executor_id: &[u8; 32]) -> Result<(), StoreError> {
        let key = hex::encode(executor_id);
        let current = self.positive.get(&key)?.as_deref().copied().unwrap_or(0);
        let new_value = current.checked_add(1).ok_or_else(|| {
            StorageError::InvalidData(
                "Counter increment overflow: value already at u64::MAX".to_owned(),
            )
        })?;
        let _previous = self.positive.insert(key, new_value)?;
        Ok(())
    }

    /// Get the positive count for a specific executor
    ///
    /// # Errors
    /// Returns error if storage operation fails
    pub fn get_positive_count(&self, executor_id: &[u8; 32]) -> Result<u64, StoreError> {
        let key = hex::encode(executor_id);
        Ok(self.positive.get(&key)?.as_deref().copied().unwrap_or(0))
    }
}

// G-Counter specific methods (ALLOW_DECREMENT = false)
impl<S: StorageAdaptor> Counter<false, S> {
    /// Get the total count across all executors (G-Counter only)
    ///
    /// Returns `u64` since G-Counter cannot go negative.
    ///
    /// # Errors
    /// Returns error if storage operation fails or if counter sum overflows u64::MAX
    pub fn value(&self) -> Result<u64, StoreError> {
        let mut total = 0u64;
        for (_key, count) in self.positive.entries()? {
            // Safe addition: check for overflow
            total = total.checked_add(count).ok_or_else(|| {
                StorageError::InvalidData("Counter sum overflow: exceeded u64::MAX".to_owned())
            })?;
        }
        Ok(total)
    }

    /// Alias for `value()` for API consistency
    ///
    /// # Errors
    /// Returns error if storage operation fails
    pub fn value_unsigned(&self) -> Result<u64, StoreError> {
        self.value()
    }
}

// PN-Counter specific methods (ALLOW_DECREMENT = true)
impl<S: StorageAdaptor> Counter<true, S> {
    /// Decrement the counter for the current executor (PN-Counter only)
    ///
    /// # Panics
    /// Panics if called inside a state migration (`#[app::migrate]`, i.e. storage
    /// merge mode): like [`increment`](Counter::increment), `decrement` stamps the
    /// running node's executor id, which differs per node and would diverge the
    /// network. Carry the counter across unchanged or use
    /// [`decrement_for`](Counter::decrement_for) with a deterministic executor id.
    ///
    /// # Errors
    /// Returns error if storage operation fails
    #[expect(
        clippy::panic,
        reason = "non-deterministic during migrate (per-node executor id); a loud panic \
                  is the intended, unmissable guard against a silent network divergence"
    )]
    pub fn decrement(&mut self) -> Result<(), StoreError> {
        if crate::env::in_merge_mode() {
            panic!(
                "Counter::decrement() is non-deterministic during a state migration: it \
                 stamps this node's identity, which differs per node and diverges the \
                 network. Carry the counter across unchanged (`field: old.field`) or \
                 replay with `decrement_for(executor_id, …)`."
            );
        }
        let executor_id = crate::env::executor_id();
        self.decrement_for(&executor_id)
    }

    /// Decrement the counter for a specific executor (PN-Counter only)
    ///
    /// # Errors
    /// Returns error if storage operation fails or if decrement would overflow u64::MAX
    pub fn decrement_for(&mut self, executor_id: &[u8; 32]) -> Result<(), StoreError> {
        let key = hex::encode(executor_id);
        let current = self.negative.get(&key)?.as_deref().copied().unwrap_or(0);
        let new_value = current.checked_add(1).ok_or_else(|| {
            StorageError::InvalidData(
                "Counter decrement overflow: value already at u64::MAX".to_owned(),
            )
        })?;
        let _previous = self.negative.insert(key, new_value)?;
        Ok(())
    }

    /// Get the negative count for a specific executor (PN-Counter only)
    ///
    /// # Errors
    /// Returns error if storage operation fails
    pub fn get_negative_count(&self, executor_id: &[u8; 32]) -> Result<u64, StoreError> {
        let key = hex::encode(executor_id);
        Ok(self.negative.get(&key)?.as_deref().copied().unwrap_or(0))
    }

    /// Get the total count across all executors (PN-Counter only)
    ///
    /// Returns `i64` since PN-Counter can go negative.
    /// Value = sum(positive) - sum(negative)
    ///
    /// # Errors
    /// Returns error if storage operation fails or if counter values overflow i64 bounds
    pub fn value_signed(&self) -> Result<i64, StoreError> {
        let mut pos_total = 0i64;
        for (_, count) in self.positive.entries()? {
            // Safe conversion: check if u64 fits in i64
            let count_i64 = i64::try_from(count).map_err(|_| {
                StorageError::InvalidData(format!(
                    "Counter value {count} exceeds i64::MAX, cannot represent in signed counter"
                ))
            })?;

            // Safe addition: check for overflow
            pos_total = pos_total.checked_add(count_i64).ok_or_else(|| {
                StorageError::InvalidData(
                    "Counter positive sum overflow: exceeded i64::MAX".to_owned(),
                )
            })?;
        }

        let mut neg_total = 0i64;
        for (_, count) in self.negative.entries()? {
            // Safe conversion: check if u64 fits in i64
            let count_i64 = i64::try_from(count).map_err(|_| {
                StorageError::InvalidData(format!(
                    "Counter value {count} exceeds i64::MAX, cannot represent in signed counter"
                ))
            })?;

            // Safe addition: check for overflow
            neg_total = neg_total.checked_add(count_i64).ok_or_else(|| {
                StorageError::InvalidData(
                    "Counter negative sum overflow: exceeded i64::MAX".to_owned(),
                )
            })?;
        }

        // Safe subtraction: check for overflow
        Ok(pos_total.checked_sub(neg_total).ok_or_else(|| {
            StorageError::InvalidData(
                "Counter final value overflow: result exceeds i64 bounds".to_owned(),
            )
        })?)
    }

    /// Alias for `value_signed()` for API consistency
    ///
    /// # Errors
    /// Returns error if storage operation fails
    pub fn value(&self) -> Result<i64, StoreError> {
        self.value_signed()
    }
}

impl<const ALLOW_DECREMENT: bool, S: StorageAdaptor> Default for Counter<ALLOW_DECREMENT, S> {
    fn default() -> Self {
        Self::new_internal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::Root;

    // ========== Migration merge-mode guard ==========

    #[test]
    #[should_panic(expected = "migration")]
    fn increment_panics_during_migration() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(GCounter::new);
        // `increment()` stamps the current node's identity; running it inside a
        // migrate body (merge mode is active) would key the delta differently on
        // every node and diverge the network. It must refuse, loudly.
        crate::env::with_merge_mode(|| {
            let _ = counter.increment();
        });
    }

    #[test]
    fn increment_for_is_allowed_during_migration() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(GCounter::new);
        // The deterministic replay API (explicit executor id) stays usable in a
        // migrate body — that is the sanctioned way to rebuild a counter.
        crate::env::with_merge_mode(|| {
            counter.increment_for(&[1u8; 32]).unwrap();
        });
        assert_eq!(counter.value().unwrap(), 1);
    }

    #[test]
    #[should_panic(expected = "migration")]
    fn decrement_panics_during_migration() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(PNCounter::new);
        // `decrement` mirrors `increment` — it stamps the running node's id, so it
        // must refuse inside a migrate (merge mode) for the same reason.
        crate::env::with_merge_mode(|| {
            let _ = counter.decrement();
        });
    }

    #[test]
    fn decrement_for_is_allowed_during_migration() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(PNCounter::new);
        crate::env::with_merge_mode(|| {
            counter.decrement_for(&[1u8; 32]).unwrap();
        });
        assert_eq!(counter.value_signed().unwrap(), -1);
    }

    // ========== G-Counter Tests ==========

    #[test]
    fn test_gcounter_increment() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(GCounter::new);
        let executor_id = [91u8; 32];

        counter.increment_for(&executor_id).unwrap();
        assert_eq!(counter.value().unwrap(), 1);

        counter.increment_for(&executor_id).unwrap();
        assert_eq!(counter.value().unwrap(), 2);

        for _ in 0..5 {
            counter.increment_for(&executor_id).unwrap();
        }
        assert_eq!(counter.value().unwrap(), 7);
    }

    #[test]
    fn test_gcounter_starts_at_zero() {
        crate::env::reset_for_testing();
        let counter = Root::new(GCounter::new);
        assert_eq!(counter.value().unwrap(), 0);
    }

    #[test]
    fn test_gcounter_multiple_executors() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(GCounter::new);
        let executor_a = [92u8; 32];
        let executor_b = [93u8; 32];

        counter.increment_for(&executor_a).unwrap();
        counter.increment_for(&executor_a).unwrap();
        counter.increment_for(&executor_b).unwrap();

        assert_eq!(counter.get_positive_count(&executor_a).unwrap(), 2);
        assert_eq!(counter.get_positive_count(&executor_b).unwrap(), 1);
        assert_eq!(counter.value().unwrap(), 3);
    }

    #[test]
    fn test_gcounter_value_unsigned() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(Counter::<false>::new);
        let executor_id = [94u8; 32];

        counter.increment_for(&executor_id).unwrap();
        counter.increment_for(&executor_id).unwrap();

        assert_eq!(counter.value_unsigned().unwrap(), 2);
        assert_eq!(counter.value().unwrap(), 2);
    }

    // ========== PN-Counter Tests ==========

    #[test]
    fn test_pncounter_increment_and_decrement() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(PNCounter::new);
        let executor_id = [95u8; 32];

        // Start at 0
        assert_eq!(counter.value().unwrap(), 0);

        // Increment
        counter.increment_for(&executor_id).unwrap();
        assert_eq!(counter.value().unwrap(), 1);

        counter.increment_for(&executor_id).unwrap();
        counter.increment_for(&executor_id).unwrap();
        assert_eq!(counter.value().unwrap(), 3);

        // Decrement
        counter.decrement_for(&executor_id).unwrap();
        assert_eq!(counter.value().unwrap(), 2);

        counter.decrement_for(&executor_id).unwrap();
        counter.decrement_for(&executor_id).unwrap();
        assert_eq!(counter.value().unwrap(), 0);

        // Go negative
        counter.decrement_for(&executor_id).unwrap();
        assert_eq!(counter.value().unwrap(), -1);

        counter.decrement_for(&executor_id).unwrap();
        assert_eq!(counter.value().unwrap(), -2);
    }

    #[test]
    fn test_pncounter_multiple_executors() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(PNCounter::new);
        let executor_a = [96u8; 32];
        let executor_b = [97u8; 32];

        // Executor A: +3
        counter.increment_for(&executor_a).unwrap();
        counter.increment_for(&executor_a).unwrap();
        counter.increment_for(&executor_a).unwrap();

        // Executor B: +2
        counter.increment_for(&executor_b).unwrap();
        counter.increment_for(&executor_b).unwrap();

        assert_eq!(counter.value().unwrap(), 5);

        // Executor A: -1
        counter.decrement_for(&executor_a).unwrap();
        assert_eq!(counter.value().unwrap(), 4);

        // Executor B: -2
        counter.decrement_for(&executor_b).unwrap();
        counter.decrement_for(&executor_b).unwrap();
        assert_eq!(counter.value().unwrap(), 2);

        // Check individual counts
        assert_eq!(counter.get_positive_count(&executor_a).unwrap(), 3);
        assert_eq!(counter.get_negative_count(&executor_a).unwrap(), 1);
        assert_eq!(counter.get_positive_count(&executor_b).unwrap(), 2);
        assert_eq!(counter.get_negative_count(&executor_b).unwrap(), 2);
    }

    #[test]
    fn test_pncounter_value_signed() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(Counter::<true>::new);
        let executor_id = [98u8; 32];

        counter.increment_for(&executor_id).unwrap();
        assert_eq!(counter.value_signed().unwrap(), 1);

        counter.decrement_for(&executor_id).unwrap();
        counter.decrement_for(&executor_id).unwrap();
        assert_eq!(counter.value_signed().unwrap(), -1);
        assert_eq!(counter.value().unwrap(), -1);
    }

    #[test]
    fn test_pncounter_starts_at_zero() {
        crate::env::reset_for_testing();
        let counter = Root::new(PNCounter::new);
        assert_eq!(counter.value().unwrap(), 0);
    }

    // ========== Overflow Tests ==========

    #[test]
    fn test_gcounter_overflow_detection() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(GCounter::new);
        let executor_id = [99u8; 32];

        // Manually insert a value near u64::MAX to trigger overflow
        let key = hex::encode(executor_id);
        counter.positive.insert(key.clone(), u64::MAX - 10).unwrap();

        // This should still work
        assert!(counter.value().is_ok());

        // Add another executor with a large value that will cause overflow
        let executor_id2 = [100u8; 32];
        let key2 = hex::encode(executor_id2);
        counter.positive.insert(key2, 100).unwrap();

        // Now value() should detect overflow and return error
        let result = counter.value();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("overflow") || err_msg.contains("u64::MAX"));
    }

    #[test]
    fn test_pncounter_cast_overflow_detection() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(PNCounter::new);
        let executor_id = [101u8; 32];

        // Manually insert a value that exceeds i64::MAX
        let key = hex::encode(executor_id);
        let invalid_value = (i64::MAX as u64) + 1;
        counter.positive.insert(key, invalid_value).unwrap();

        // value_signed() should detect the overflow during u64 -> i64 conversion
        let result = counter.value_signed();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("i64::MAX") || err_msg.contains("overflow"));
    }

    #[test]
    fn test_pncounter_addition_overflow_detection() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(PNCounter::new);

        // Insert two values that individually fit in i64 but sum > i64::MAX
        let executor_a = [102u8; 32];
        let executor_b = [103u8; 32];

        let key_a = hex::encode(executor_a);
        let key_b = hex::encode(executor_b);

        let large_value = i64::MAX / 2 + 1;
        counter.positive.insert(key_a, large_value as u64).unwrap();
        counter.positive.insert(key_b, large_value as u64).unwrap();

        // value_signed() should detect overflow during addition
        let result = counter.value_signed();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("overflow") || err_msg.contains("i64::MAX"));
    }

    #[test]
    fn test_pncounter_subtraction_overflow_detection() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(PNCounter::new);

        // Create a scenario where pos - neg would underflow i64::MIN
        let executor_id = [104u8; 32];
        let key = hex::encode(executor_id);

        // Set positive to 0 and negative to i64::MAX
        // This should result in trying to compute 0 - i64::MAX = i64::MIN - 1 (underflow)
        counter.positive.insert(key.clone(), 0).unwrap();
        counter.negative.insert(key, i64::MAX as u64).unwrap();

        // This particular case actually works (0 - i64::MAX = i64::MIN + 1)
        // Let's try a worse case: small positive, very large negative
        let executor_id2 = [105u8; 32];
        let key2 = hex::encode(executor_id2);
        counter.negative.insert(key2, i64::MAX as u64 + 1).unwrap();

        // This should fail at the cast stage (negative value > i64::MAX)
        let result = counter.value_signed();
        assert!(result.is_err());
    }

    #[test]
    fn test_gcounter_no_false_positives() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(GCounter::new);

        // Add some large but valid values
        for i in 0..10 {
            let executor_id = [106u8 + i; 32];
            let key = hex::encode(executor_id);
            counter.positive.insert(key, 1_000_000_000).unwrap();
        }

        // Should not overflow with reasonable values
        assert!(counter.value().is_ok());
        assert_eq!(counter.value().unwrap(), 10_000_000_000);
    }

    #[test]
    fn test_pncounter_no_false_positives() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(PNCounter::new);

        // Add some large but valid values
        let executor_a = [116u8; 32];
        let executor_b = [117u8; 32];
        let key_a = hex::encode(executor_a);
        let key_b = hex::encode(executor_b);

        // Values that should work fine
        counter
            .positive
            .insert(key_a.clone(), 1_000_000_000_000)
            .unwrap();
        counter.negative.insert(key_b, 500_000_000_000).unwrap();

        // Should not overflow
        assert!(counter.value_signed().is_ok());
        assert_eq!(counter.value_signed().unwrap(), 500_000_000_000);
    }

    #[test]
    fn test_type_safety_pncounter_to_gcounter() {
        crate::env::reset_for_testing();

        // Create a PNCounter with 10 increments and 3 decrements (value = 7)
        let mut pn_counter = PNCounter::new();
        let executor_id = [120u8; 32];

        for _ in 0..10 {
            pn_counter.increment_for(&executor_id).unwrap();
        }
        for _ in 0..3 {
            pn_counter.decrement_for(&executor_id).unwrap();
        }

        assert_eq!(pn_counter.value().unwrap(), 7);

        // Serialize the PNCounter
        let serialized = borsh::to_vec(&pn_counter).unwrap();

        // Try to deserialize as GCounter - this will fail
        let result: Result<GCounter, _> = borsh::from_slice(&serialized);

        // Borsh detects extra bytes in stream (the negative map) and fails
        // This prevents silent data loss when using the wrong type
        assert!(
            result.is_err(),
            "Deserializing PNCounter as GCounter should fail"
        );

        // The error indicates leftover data
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("Not all bytes read") || err_str.contains("Unexpected length"),
            "Error should indicate leftover data, got: {err_str}"
        );
    }

    #[test]
    fn test_type_safety_gcounter_to_pncounter() {
        crate::env::reset_for_testing();

        // Create a GCounter with 10 increments
        let mut g_counter = GCounter::new();
        let executor_id = [121u8; 32];

        for _ in 0..10 {
            g_counter.increment_for(&executor_id).unwrap();
        }

        assert_eq!(g_counter.value().unwrap(), 10);

        // Serialize the GCounter
        let serialized = borsh::to_vec(&g_counter).unwrap();

        // Deserialize as PNCounter - this should work (upgrading is safe)
        let pn_counter: PNCounter = borsh::from_slice(&serialized).unwrap();

        // Should have the same value
        assert_eq!(pn_counter.value().unwrap(), 10);
    }

    #[test]
    fn test_exact_scenario_from_issue() {
        crate::env::reset_for_testing();

        // The exact scenario from the issue description:
        // PNCounter with 10 increments and 3 decrements (value=7)
        let mut pn_counter = PNCounter::new();
        let executor_id = [122u8; 32];

        // 10 increments
        for _ in 0..10 {
            pn_counter.increment_for(&executor_id).unwrap();
        }

        // 3 decrements
        for _ in 0..3 {
            pn_counter.decrement_for(&executor_id).unwrap();
        }

        // Value should be 7
        assert_eq!(pn_counter.value().unwrap(), 7);

        // Serialize the PNCounter
        let serialized = borsh::to_vec(&pn_counter).unwrap();

        // Try to deserialize as GCounter
        let result: Result<GCounter, _> = borsh::from_slice(&serialized);

        // This SHOULD fail - prevents the issue where value would incorrectly be 10
        // Borsh detects leftover bytes (the negative map) and rejects the deserialization
        assert!(
            result.is_err(),
            "Should prevent deserializing PNCounter(7) as GCounter(10)"
        );

        // Verify the error indicates extra data in the stream
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("Not all bytes read") || err_str.contains("Unexpected length"),
            "Error should indicate leftover data, got: {err_str}"
        );
    }

    #[test]
    fn test_deterministic_counter_ids() {
        crate::env::reset_for_testing();

        // Create two counters with the same field name - they should have the same positive map IDs
        let counter1 = GCounter::new_with_field_name("visit_count");
        let counter2 = GCounter::new_with_field_name("visit_count");

        assert_eq!(
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter1.positive),
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter2.positive),
            "Counters with same field name should have same positive map ID"
        );

        // Note: GCounter's negative map uses new_detached() and has random IDs.
        // This is intentional - the negative map is never used for GCounter, so its ID
        // doesn't matter. Using detached prevents adding useless children to ROOT_ID.
        // Only PNCounter needs deterministic negative map IDs.

        // Different field names should produce different positive map IDs
        let counter3 = GCounter::new_with_field_name("click_count");
        assert_ne!(
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter1.positive),
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter3.positive),
            "Counters with different field names should have different IDs"
        );
    }

    #[test]
    fn test_pncounter_deterministic_ids() {
        crate::env::reset_for_testing();

        // For PNCounter, BOTH positive and negative maps should have deterministic IDs
        let counter1 = PNCounter::new_with_field_name("balance");
        let counter2 = PNCounter::new_with_field_name("balance");

        assert_eq!(
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter1.positive),
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter2.positive),
            "PNCounters with same field name should have same positive map ID"
        );
        assert_eq!(
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter1.negative),
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter2.negative),
            "PNCounters with same field name should have same negative map ID"
        );
    }

    #[test]
    fn test_random_vs_deterministic_counter_ids() {
        crate::env::reset_for_testing();

        // Random IDs (new()) should be different each time
        let counter1 = GCounter::new();
        let counter2 = GCounter::new();

        assert_ne!(
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter1.positive),
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter2.positive),
            "Counters with new() should have different random IDs"
        );

        // Deterministic IDs (new_with_field_name) should be the same
        let counter3 = GCounter::new_with_field_name("visits");
        let counter4 = GCounter::new_with_field_name("visits");
        assert_eq!(
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter3.positive),
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter4.positive),
            "Counters with same field name should have same ID"
        );
    }

    #[test]
    fn test_counter_internal_maps_no_collision() {
        crate::env::reset_for_testing();

        // Verify that Counter's internal maps don't collide with user-created collections
        // A Counter with field name "visits" should NOT collide with a user-created
        // UnorderedMap with field name "visits_positive" or "visits_negative"
        let counter = GCounter::new_with_field_name("visits");
        let user_map_positive = UnorderedMap::<String, u64>::new_with_field_name("visits_positive");
        let user_map_negative = UnorderedMap::<String, u64>::new_with_field_name("visits_negative");

        // Counter's internal maps should have different IDs than user-created maps
        assert_ne!(
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter.positive),
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&user_map_positive),
            "Counter's internal positive map should not collide with user-created 'visits_positive' map"
        );
        assert_ne!(
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter.negative),
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&user_map_negative),
            "Counter's internal negative map should not collide with user-created 'visits_negative' map"
        );

        // Also verify that Counter's internal maps use the reserved prefix
        // by checking that a map with the actual internal name matches
        let internal_positive =
            UnorderedMap::<String, u64>::new_with_field_name("__counter_internal_visits_positive");
        assert_eq!(
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&counter.positive),
            <UnorderedMap<String, u64> as crate::entities::Data>::id(&internal_positive),
            "Counter's internal positive map should use the reserved prefix"
        );
    }
}
