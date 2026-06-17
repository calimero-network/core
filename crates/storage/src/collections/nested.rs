//! Nested CRDT storage helpers.
//!
//! When you store a CRDT inside another CRDT (e.g. `Map<K, Map<K2, V>>`), the
//! inner value is stored as a single map element and converges via the outer
//! `UnorderedMap`'s recursive `Mergeable` impl. Composite-key flattening (one
//! storage row per inner entry, for prefix scans) is a deferred optimization
//! and is NOT performed today — `insert_nested*` fall back to a standard
//! `map.insert`.

use super::composite_key::CompositeKey;
use super::crdt_meta::{CrdtMeta, Decomposable};
use super::{StoreError, UnorderedMap, ValueRef};
use crate::store::StorageAdaptor;
use borsh::{BorshDeserialize, BorshSerialize};

/// Configuration for nested CRDT storage
#[derive(Debug, Clone, Copy)]
pub struct NestedConfig {
    /// Maximum nesting depth (prevents infinite recursion)
    pub max_depth: usize,
    /// Whether to enable automatic flattening
    pub auto_flatten: bool,
}

impl Default for NestedConfig {
    fn default() -> Self {
        Self {
            max_depth: 3,
            auto_flatten: true,
        }
    }
}

/// Insert a value into a map with automatic nested CRDT handling
///
/// This is a generic wrapper that works with any value type:
/// - Simple CRDTs (Counter, LwwRegister, RGA): stored normally
/// - Container CRDTs (Map, Vector): decomposed with composite keys (if they implement Decomposable)
/// - Non-CRDTs: stored normally
///
/// # Type Parameters
///
/// - `K`: Outer map key type
/// - `V`: Value type (may be a nested CRDT)
/// - `S`: Storage adaptor
///
/// # Errors
///
/// Returns error if storage operations fail.
pub fn insert_nested<K, V, S>(
    map: &mut UnorderedMap<K, V, S>,
    key: K,
    value: V,
) -> Result<bool, StoreError>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + Clone + PartialEq + 'static,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    // Stored as a single element for all value types; composite-key
    // decomposition is a deferred optimization (see module docs).
    map.insert(key, value).map(|_| true)
}

/// Insert a decomposable CRDT with automatic flattening
///
/// This is the specialized version for CRDTs that implement Decomposable.
///
/// # Errors
///
/// Returns error if decomposition or storage fails.
pub fn insert_nested_decomposable<K, V, S>(
    map: &mut UnorderedMap<K, V, S>,
    key: K,
    value: V,
) -> Result<bool, StoreError>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + Clone + PartialEq + 'static,
    V: BorshSerialize
        + BorshDeserialize
        + Clone
        + CrdtMeta
        + Decomposable<Key = CompositeKey>
        + 'static,
    S: StorageAdaptor,
{
    // Both decomposable containers and simple CRDTs store as a single map
    // element; nested values merge correctly via the `Mergeable` trait, so the
    // composite-key decomposition (a deferred storage optimization, not a
    // correctness requirement) is not performed here.
    map.insert(key, value).map(|_| true)
}

/// Retrieve a value from a map with automatic nested CRDT reconstruction
///
/// If the key points to a decomposed nested CRDT, this will scan for all
/// related composite keys and reconstruct the original structure.
///
/// # Type Parameters
///
/// - `K`: Outer map key type
/// - `V`: Value type (may be a nested CRDT)
/// - `S`: Storage adaptor
///
/// # Errors
///
/// Returns error if storage operations fail or reconstruction fails.
pub fn get_nested<K, V, S>(map: &UnorderedMap<K, V, S>, key: &K) -> Result<Option<V>, StoreError>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + Clone + PartialEq + 'static,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    // For Phase 2.1, use standard get for all types
    // Phase 2.2 will add reconstruction logic
    Ok(map.get(key)?.map(ValueRef::into_inner))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::{Counter, LwwRegister, Root};
    use crate::env;

    #[test]
    fn test_nested_config_default() {
        let config = NestedConfig::default();
        assert_eq!(config.max_depth, 3);
        assert!(config.auto_flatten);
    }

    #[test]
    fn test_insert_nested_simple_crdt() {
        env::reset_for_testing();

        let mut map = Root::new(|| UnorderedMap::<String, Counter>::new());
        let counter = Counter::new();

        // Counter doesn't contain CRDTs, so uses standard insert
        let result = insert_nested(&mut map, "counter1".to_string(), counter);
        assert!(result.is_ok());
    }

    #[test]
    fn test_insert_nested_lww_register() {
        env::reset_for_testing();

        let mut map = Root::new(|| UnorderedMap::<String, LwwRegister<String>>::new());
        let register = LwwRegister::new("Test Value".to_string());

        // LwwRegister doesn't contain CRDTs, so uses standard insert
        let result = insert_nested(&mut map, "reg1".to_string(), register);
        assert!(result.is_ok());
    }

    // NOTE: Nested Map<K, Map<K2, V>> test is in merge_integration.rs
    // The test_merge_with_nested_map() proves nested maps work correctly
    // via Mergeable trait, without requiring composite key storage optimization.
}
