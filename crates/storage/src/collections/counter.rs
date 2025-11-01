//! Counter collection - CRDT-compatible counter supporting both G-Counter and PN-Counter modes.
//!
//! This module provides a unified counter implementation that can operate as either:
//! - **G-Counter** (Grow-Only Counter): Increment-only, lighter weight (default)
//! - **PN-Counter** (Positive-Negative Counter): Supports both increment and decrement
//!
//! The mode is selected at compile-time using const generics for zero runtime overhead.

use borsh::io::{Error, ErrorKind, Read, Result as BorshResult, Write};
use borsh::{BorshDeserialize, BorshSerialize};

use super::{StorageAdaptor, UnorderedMap};
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

// Custom deserialization: validate counter type safety
//
// Deserialization behavior:
//
// 1. GCounter ← GCounter: ✅ Works perfectly
// 2. GCounter ← PNCounter (empty negative): ✅ Works (no data loss)
// 3. GCounter ← PNCounter (has negative): ❌ ERROR - prevents silent data loss
// 4. PNCounter ← GCounter: ✅ Safe upgrade (initializes empty negative map)
// 5. PNCounter ← PNCounter: ✅ Works perfectly
//
// This ensures type safety while allowing safe upgrades from GCounter to PNCounter.
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
                Err(_) => {
                    // No negative field present - this is a GCounter being upgraded to PNCounter
                    // This is a safe operation since GCounter has no negative counts
                    UnorderedMap::new_internal()
                }
            }
        } else {
            // GCounter: Check if there's more data (which would indicate a PNCounter being deserialized as GCounter)
            // We peek ahead by trying to deserialize a negative map
            match UnorderedMap::<String, u64, S>::deserialize_reader(reader) {
                Ok(neg_map) => {
                    // Successfully deserialized negative counts - this is a PNCounter!
                    // Check if it has any entries (non-zero negative counts)
                    if neg_map.len().unwrap_or(0) > 0 {
                        return Err(Error::new(
                            ErrorKind::InvalidData,
                            "Cannot deserialize PNCounter with negative counts as GCounter. \
                             This would silently drop decrement data and produce incorrect values. \
                             Use PNCounter instead or ensure the source counter has no decrements.",
                        ));
                    }
                    // Empty negative map is OK - might be deserializing a PNCounter with no decrements yet
                    neg_map
                }
                Err(_) => {
                    // No more data - this is truly a GCounter serialized data
                    UnorderedMap::new_internal()
                }
            }
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
    /// Creates a new counter
    #[must_use]
    pub fn new() -> Self {
        Self::new_internal()
    }
}

impl<const ALLOW_DECREMENT: bool, S: StorageAdaptor> Counter<ALLOW_DECREMENT, S> {
    /// Creates a new counter (internal) - must use same visibility as UnorderedMap
    pub(super) fn new_internal() -> Self {
        Self {
            positive: UnorderedMap::new_internal(),
            negative: UnorderedMap::new_internal(),
        }
    }

    /// Increment the counter for the current executor
    ///
    /// # Errors
    /// Returns error if storage operation fails
    pub fn increment(&mut self) -> Result<(), StoreError> {
        let executor_id = crate::env::executor_id();
        self.increment_for(&executor_id)
    }

    /// Increment the counter for a specific executor
    ///
    /// # Errors
    /// Returns error if storage operation fails
    pub fn increment_for(&mut self, executor_id: &[u8; 32]) -> Result<(), StoreError> {
        let key = hex::encode(executor_id);
        let current = self.positive.get(&key)?.unwrap_or(0);
        let _previous = self.positive.insert(key, current + 1)?;
        Ok(())
    }

    /// Get the positive count for a specific executor
    ///
    /// # Errors
    /// Returns error if storage operation fails
    pub fn get_positive_count(&self, executor_id: &[u8; 32]) -> Result<u64, StoreError> {
        let key = hex::encode(executor_id);
        Ok(self.positive.get(&key)?.unwrap_or(0))
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
        for (_, count) in self.positive.entries()? {
            // Safe addition: check for overflow
            total = total.checked_add(count).ok_or_else(|| {
                StorageError::InvalidData("Counter sum overflow: exceeded u64::MAX".to_string())
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
    /// # Errors
    /// Returns error if storage operation fails
    pub fn decrement(&mut self) -> Result<(), StoreError> {
        let executor_id = crate::env::executor_id();
        self.decrement_for(&executor_id)
    }

    /// Decrement the counter for a specific executor (PN-Counter only)
    ///
    /// # Errors
    /// Returns error if storage operation fails
    pub fn decrement_for(&mut self, executor_id: &[u8; 32]) -> Result<(), StoreError> {
        let key = hex::encode(executor_id);
        let current = self.negative.get(&key)?.unwrap_or(0);
        let _previous = self.negative.insert(key, current + 1)?;
        Ok(())
    }

    /// Get the negative count for a specific executor (PN-Counter only)
    ///
    /// # Errors
    /// Returns error if storage operation fails
    pub fn get_negative_count(&self, executor_id: &[u8; 32]) -> Result<u64, StoreError> {
        let key = hex::encode(executor_id);
        Ok(self.negative.get(&key)?.unwrap_or(0))
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
                    "Counter value {} exceeds i64::MAX, cannot represent in signed counter",
                    count
                ))
            })?;

            // Safe addition: check for overflow
            pos_total = pos_total.checked_add(count_i64).ok_or_else(|| {
                StorageError::InvalidData(
                    "Counter positive sum overflow: exceeded i64::MAX".to_string(),
                )
            })?;
        }

        let mut neg_total = 0i64;
        for (_, count) in self.negative.entries()? {
            // Safe conversion: check if u64 fits in i64
            let count_i64 = i64::try_from(count).map_err(|_| {
                StorageError::InvalidData(format!(
                    "Counter value {} exceeds i64::MAX, cannot represent in signed counter",
                    count
                ))
            })?;

            // Safe addition: check for overflow
            neg_total = neg_total.checked_add(count_i64).ok_or_else(|| {
                StorageError::InvalidData(
                    "Counter negative sum overflow: exceeded i64::MAX".to_string(),
                )
            })?;
        }

        // Safe subtraction: check for overflow
        Ok(pos_total.checked_sub(neg_total).ok_or_else(|| {
            StorageError::InvalidData(
                "Counter final value overflow: result exceeds i64 bounds".to_string(),
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

    // ========== G-Counter Tests ==========

    #[test]
    fn test_gcounter_increment() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(|| GCounter::new());
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
        let counter = Root::new(|| GCounter::new());
        assert_eq!(counter.value().unwrap(), 0);
    }

    #[test]
    fn test_gcounter_multiple_executors() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(|| GCounter::new());
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
        let mut counter = Root::new(|| Counter::<false>::new());
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
        let mut counter = Root::new(|| PNCounter::new());
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
        let mut counter = Root::new(|| PNCounter::new());
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
        let mut counter = Root::new(|| Counter::<true>::new());
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
        let counter = Root::new(|| PNCounter::new());
        assert_eq!(counter.value().unwrap(), 0);
    }

    // ========== Overflow Tests ==========

    #[test]
    fn test_gcounter_overflow_detection() {
        crate::env::reset_for_testing();
        let mut counter = Root::new(|| GCounter::new());
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
        let mut counter = Root::new(|| PNCounter::new());
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
        let mut counter = Root::new(|| PNCounter::new());

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
        let mut counter = Root::new(|| PNCounter::new());

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
        let mut counter = Root::new(|| GCounter::new());

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
        let mut counter = Root::new(|| PNCounter::new());

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

        // Try to deserialize as GCounter - this should fail with the fix
        let result: Result<GCounter, _> = borsh::from_slice(&serialized);

        // With the fix, this should fail because we can't deserialize
        // a PNCounter (which has negative counts) as a GCounter
        assert!(
            result.is_err(),
            "Deserializing a PNCounter with negative counts as GCounter should fail"
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

        // This MUST fail (prevents the issue where value would incorrectly be 10)
        assert!(
            result.is_err(),
            "Should prevent deserializing PNCounter(7) as GCounter(10)"
        );

        // Verify the error message is informative
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Cannot deserialize PNCounter"));
        assert!(err.to_string().contains("silently drop"));
    }
}
