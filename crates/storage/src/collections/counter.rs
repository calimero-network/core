//! Counter collection - CRDT-compatible counter supporting both G-Counter and PN-Counter modes.
//!
//! This module provides a unified counter implementation that can operate as either:
//! - **G-Counter** (Grow-Only Counter): Increment-only, lighter weight (default)
//! - **PN-Counter** (Positive-Negative Counter): Supports both increment and decrement
//!
//! The mode is selected at compile-time using const generics for zero runtime overhead.

use borsh::{BorshDeserialize, BorshSerialize};

use super::{StorageAdaptor, UnorderedMap};
use crate::collections::error::StoreError;
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
#[derive(BorshSerialize, BorshDeserialize, Debug)]
#[borsh(crate = "borsh")]
pub struct Counter<const ALLOW_DECREMENT: bool = false, S: StorageAdaptor = MainStorage> {
    /// Maps executor_id (hex string) -> positive increments
    #[borsh(bound(serialize = "", deserialize = ""))]
    pub(crate) positive: UnorderedMap<String, u64, S>,

    /// Maps executor_id (hex string) -> negative decrements
    /// Only used when ALLOW_DECREMENT = true
    #[borsh(bound(serialize = "", deserialize = ""))]
    pub(crate) negative: UnorderedMap<String, u64, S>,
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
    /// Returns error if storage operation fails
    pub fn value(&self) -> Result<u64, StoreError> {
        let mut total = 0u64;
        for (_, count) in self.positive.entries()? {
            total += count;
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
    /// Returns error if storage operation fails
    pub fn value_signed(&self) -> Result<i64, StoreError> {
        let mut pos_total = 0i64;
        for (_, count) in self.positive.entries()? {
            pos_total += count as i64;
        }

        let mut neg_total = 0i64;
        for (_, count) in self.negative.entries()? {
            neg_total += count as i64;
        }

        Ok(pos_total - neg_total)
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
}
