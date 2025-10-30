//! Merge registry for automatic CRDT merging
//!
//! This module provides a type registry that allows merge_root_state()
//! to automatically call the correct merge logic for any app state type.
//!
//! # Problem
//!
//! The root state can be any type defined by the app. We can't know at compile
//! time what type to deserialize to. We need runtime type dispatch.
//!
//! # Solution
//!
//! Apps register their state type with a merge function:
//!
//! ```ignore
//! // In app initialization:
//! register_crdt_merge::<MyAppState>();
//!
//! // Now sync automatically calls MyAppState::merge()
//! ```

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

/// Function signature for merging serialized state
pub type MergeFn = fn(&[u8], &[u8], u64, u64) -> Result<Vec<u8>, Box<dyn std::error::Error>>;

/// Global registry of merge functions by type
static MERGE_REGISTRY: LazyLock<RwLock<HashMap<TypeId, MergeFn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a CRDT merge function for a type
///
/// # Example
///
/// ```ignore
/// #[derive(BorshSerialize, BorshDeserialize)]
/// struct MyState {
///     counter: Counter,
///     metadata: UnorderedMap<String, String>,
/// }
///
/// impl Mergeable for MyState {
///     fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
///         self.counter.merge(&other.counter)?;
///         self.metadata.merge(&other.metadata)?;
///         Ok(())
///     }
/// }
///
/// // Register at app startup
/// register_crdt_merge::<MyState>();
/// ```
pub fn register_crdt_merge<T>()
where
    T: borsh::BorshSerialize + borsh::BorshDeserialize + crate::collections::Mergeable + 'static,
{
    let type_id = TypeId::of::<T>();

    let merge_fn: MergeFn = |existing, incoming, _existing_ts, _incoming_ts| {
        // Deserialize both states
        let mut existing_state = borsh::from_slice::<T>(existing)
            .map_err(|e| format!("Failed to deserialize existing state: {}", e))?;

        let incoming_state = borsh::from_slice::<T>(incoming)
            .map_err(|e| format!("Failed to deserialize incoming state: {}", e))?;

        // Merge using Mergeable trait
        existing_state
            .merge(&incoming_state)
            .map_err(|e| format!("Merge failed: {}", e))?;

        // Serialize result
        borsh::to_vec(&existing_state).map_err(|e| format!("Serialization failed: {}", e).into())
    };

    let mut registry = MERGE_REGISTRY.write().unwrap_or_else(|_| {
        // Lock poisoning is a programming error that should never happen
        // In production, this indicates a bug in the merge system
        std::process::abort()
    });
    let _ = registry.insert(type_id, merge_fn);
}

/// Clear the merge registry (for testing only)
#[cfg(test)]
pub fn clear_merge_registry() {
    let mut registry = MERGE_REGISTRY
        .write()
        .unwrap_or_else(|_| std::process::abort());
    registry.clear();
}

/// Try to merge using registered merge function
///
/// If the type is registered, uses its merge function.
/// Otherwise, returns None to indicate fallback to LWW.
pub fn try_merge_registered(
    existing: &[u8],
    incoming: &[u8],
    existing_ts: u64,
    incoming_ts: u64,
) -> Option<Result<Vec<u8>, Box<dyn std::error::Error>>> {
    // For now, we don't have type information at runtime
    // This will be solved in Phase 3 with type hints in storage

    // Try each registered merge function (brute force for Phase 2)
    let registry = MERGE_REGISTRY.read().ok()?;

    for (_type_id, merge_fn) in registry.iter() {
        if let Ok(merged) = merge_fn(existing, incoming, existing_ts, incoming_ts) {
            return Some(Ok(merged));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::{Counter, Mergeable};
    use crate::env;

    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize, Debug)]
    struct TestState {
        counter: Counter,
    }

    impl Mergeable for TestState {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            self.counter.merge(&other.counter)
        }
    }

    #[test]
    fn test_register_and_merge() {
        env::reset_for_testing();

        // Register the type
        register_crdt_merge::<TestState>();

        // Create two states with different executor IDs (use unique IDs to avoid test contamination)
        env::set_executor_id([10; 32]);
        let mut state1 = TestState {
            counter: Counter::new(),
        };
        state1.counter.increment().unwrap();
        state1.counter.increment().unwrap(); // value = 2

        env::set_executor_id([20; 32]);
        let mut state2 = TestState {
            counter: Counter::new(),
        };
        state2.counter.increment().unwrap(); // value = 1

        // Serialize
        let bytes1 = borsh::to_vec(&state1).unwrap();
        let bytes2 = borsh::to_vec(&state2).unwrap();

        // Merge via registry
        let merged_bytes = try_merge_registered(&bytes1, &bytes2, 100, 200)
            .unwrap()
            .unwrap();

        // Deserialize result
        let merged: TestState = borsh::from_slice(&merged_bytes).unwrap();

        // Verify: counters summed (2 + 1 = 3)
        assert_eq!(merged.counter.value().unwrap(), 3);
    }
}
