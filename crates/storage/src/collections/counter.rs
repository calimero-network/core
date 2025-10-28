//! G-Counter collection - CRDT-compatible increment-only counter.
//!
//! A simple wrapper around UnorderedMap that tracks per-executor counts
//! for CRDT-safe concurrent increments.

use borsh::{BorshDeserialize, BorshSerialize};

use super::{StorageAdaptor, UnorderedMap};
use crate::collections::error::StoreError;
use crate::store::MainStorage;

/// A CRDT-compatible counter that stores per-executor increments.
///
/// This is a wrapper around `UnorderedMap<String, u64>` that provides
/// increment and sum operations for a G-Counter.
///
/// # Example
///
/// ```ignore
/// use calimero_storage::collections::Counter;
///
/// #[app::state]
/// struct MyApp {
///     visit_count: Counter,
/// }
///
/// impl MyApp {
///     pub fn increment_visitor(&mut self) {
///         self.visit_count.increment()?;
///     }
///     
///     pub fn total_visits(&self) -> u64 {
///         self.visit_count.value()?
///     }
/// }
/// ```
#[derive(BorshSerialize, BorshDeserialize, Debug)]
#[borsh(crate = "borsh")]
pub struct Counter<S: StorageAdaptor = MainStorage> {
    /// Maps executor_id (hex string) -> count
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: UnorderedMap<String, u64, S>,
}

impl Counter<MainStorage> {
    /// Creates a new counter
    #[must_use]
    pub fn new() -> Self {
        Self::new_internal()
    }
}

impl<S: StorageAdaptor> Counter<S> {
    /// Creates a new counter (internal) - must use same visibility as UnorderedMap
    pub(super) fn new_internal() -> Self {
        // Delegate to UnorderedMap's constructor
        Self {
            inner: UnorderedMap::new_internal(),
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
        let current = self.inner.get(&key)?.unwrap_or(0);
        let _previous = self.inner.insert(key, current + 1)?;
        Ok(())
    }

    /// Get the total count across all executors
    ///
    /// # Errors
    /// Returns error if storage operation fails
    pub fn value(&self) -> Result<u64, StoreError> {
        let mut total = 0;
        for (_, count) in self.inner.entries()? {
            total += count;
        }
        Ok(total)
    }

    /// Get the count for a specific executor
    ///
    /// # Errors
    /// Returns error if storage operation fails
    pub fn get_count(&self, executor_id: &[u8; 32]) -> Result<u64, StoreError> {
        let key = hex::encode(executor_id);
        Ok(self.inner.get(&key)?.unwrap_or(0))
    }
}

impl<S: StorageAdaptor> Default for Counter<S> {
    fn default() -> Self {
        Self::new_internal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::Root;

    #[test]
    fn test_counter_increment() {
        let mut counter = Root::new(|| Counter::new());
        let executor_id = [1u8; 32];

        // Increment
        counter.increment_for(&executor_id).unwrap();
        assert_eq!(counter.value().unwrap(), 1);

        // Increment again
        counter.increment_for(&executor_id).unwrap();
        assert_eq!(counter.value().unwrap(), 2);

        // Multiple increments
        for _ in 0..5 {
            counter.increment_for(&executor_id).unwrap();
        }
        assert_eq!(counter.value().unwrap(), 7);
    }

    #[test]
    fn test_counter_starts_at_zero() {
        let counter = Root::new(|| Counter::new());
        assert_eq!(counter.value().unwrap(), 0);
    }

    #[test]
    fn test_counter_multiple_executors() {
        let mut counter = Root::new(|| Counter::new());
        let executor_a = [1u8; 32];
        let executor_b = [2u8; 32];

        counter.increment_for(&executor_a).unwrap();
        counter.increment_for(&executor_a).unwrap();
        counter.increment_for(&executor_b).unwrap();

        assert_eq!(counter.get_count(&executor_a).unwrap(), 2);
        assert_eq!(counter.get_count(&executor_b).unwrap(), 1);
        assert_eq!(counter.value().unwrap(), 3);
    }
}
