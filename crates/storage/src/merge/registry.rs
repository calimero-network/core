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
//!
//! # Type-Name-Based Dispatch
//!
//! For `CrdtType::Custom { type_name }`, we support lookup by type name:
//!
//! ```ignore
//! // Registration stores both TypeId and type name
//! register_crdt_merge::<MyAppState>();
//!
//! // During sync, lookup by type name (from CrdtType::Custom)
//! try_merge_by_type_name("MyAppState", local_data, remote_data, ts1, ts2);
//! ```

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

/// Function signature for merging serialized state
pub type MergeFn = fn(&[u8], &[u8], u64, u64) -> Result<Vec<u8>, Box<dyn std::error::Error>>;

/// Registry entry with merge function
struct MergeEntry {
    merge_fn: MergeFn,
    type_name: String,
}

/// Global registry of merge functions by TypeId
static MERGE_REGISTRY: LazyLock<RwLock<HashMap<TypeId, MergeEntry>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Global registry of merge functions by type name (for CrdtType::Custom dispatch)
static NAME_REGISTRY: LazyLock<RwLock<HashMap<String, MergeFn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a CRDT merge function for a type
///
/// This registers the merge function both by `TypeId` (for in-process dispatch)
/// and by type name (for `CrdtType::Custom { type_name }` dispatch).
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
    let type_name = std::any::type_name::<T>().to_owned();

    // Extract simple type name (remove module path for matching)
    let simple_name = type_name
        .rsplit("::")
        .next()
        .unwrap_or(&type_name)
        .to_owned();

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

    // Register by TypeId
    {
        let mut registry = MERGE_REGISTRY
            .write()
            .unwrap_or_else(|_| std::process::abort());
        let _ = registry.insert(
            type_id,
            MergeEntry {
                merge_fn,
                type_name: simple_name.clone(),
            },
        );
    }

    // Register by type name (for CrdtType::Custom dispatch)
    {
        let mut name_registry = NAME_REGISTRY
            .write()
            .unwrap_or_else(|_| std::process::abort());
        let _ = name_registry.insert(simple_name, merge_fn);
    }
}

/// Clear the merge registry (for testing only)
#[cfg(test)]
pub fn clear_merge_registry() {
    {
        let mut registry = MERGE_REGISTRY
            .write()
            .unwrap_or_else(|_| std::process::abort());
        registry.clear();
    }
    {
        let mut name_registry = NAME_REGISTRY
            .write()
            .unwrap_or_else(|_| std::process::abort());
        name_registry.clear();
    }
}

/// Try to merge using registered merge function (brute force)
///
/// If the type is registered, uses its merge function.
/// Otherwise, returns None to indicate fallback to LWW.
///
/// Note: This tries each registered function until one succeeds.
/// For type-name-based dispatch (more efficient), use `try_merge_by_type_name`.
pub fn try_merge_registered(
    existing: &[u8],
    incoming: &[u8],
    existing_ts: u64,
    incoming_ts: u64,
) -> Option<Result<Vec<u8>, Box<dyn std::error::Error>>> {
    let registry = MERGE_REGISTRY.read().ok()?;

    for (_type_id, entry) in registry.iter() {
        if let Ok(merged) = (entry.merge_fn)(existing, incoming, existing_ts, incoming_ts) {
            return Some(Ok(merged));
        }
    }

    None
}

/// Try to merge using type name (for CrdtType::Custom dispatch)
///
/// This is more efficient than `try_merge_registered` because it looks up
/// directly by type name instead of trying all registered functions.
///
/// # Arguments
/// * `type_name` - The type name from `CrdtType::Custom { type_name }`
/// * `existing` - Existing serialized state
/// * `incoming` - Incoming serialized state
/// * `existing_ts` - Timestamp of existing state
/// * `incoming_ts` - Timestamp of incoming state
///
/// # Returns
/// * `Some(Ok(merged))` - Merge succeeded
/// * `Some(Err(e))` - Merge function found but failed
/// * `None` - No merge function registered for this type name
pub fn try_merge_by_type_name(
    type_name: &str,
    existing: &[u8],
    incoming: &[u8],
    existing_ts: u64,
    incoming_ts: u64,
) -> Option<Result<Vec<u8>, Box<dyn std::error::Error>>> {
    let name_registry = NAME_REGISTRY.read().ok()?;

    if let Some(merge_fn) = name_registry.get(type_name) {
        return Some(merge_fn(existing, incoming, existing_ts, incoming_ts));
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
        clear_merge_registry();

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

    #[test]
    fn test_merge_by_type_name() {
        env::reset_for_testing();
        clear_merge_registry();

        // Register the type
        register_crdt_merge::<TestState>();

        // Create two states
        env::set_executor_id([30; 32]);
        let mut state1 = TestState {
            counter: Counter::new(),
        };
        state1.counter.increment().unwrap();
        state1.counter.increment().unwrap();
        state1.counter.increment().unwrap(); // value = 3

        env::set_executor_id([40; 32]);
        let mut state2 = TestState {
            counter: Counter::new(),
        };
        state2.counter.increment().unwrap();
        state2.counter.increment().unwrap(); // value = 2

        let bytes1 = borsh::to_vec(&state1).unwrap();
        let bytes2 = borsh::to_vec(&state2).unwrap();

        // Merge via type name (efficient lookup)
        let merged_bytes = try_merge_by_type_name("TestState", &bytes1, &bytes2, 100, 200)
            .expect("Should find registered type")
            .expect("Merge should succeed");

        let merged: TestState = borsh::from_slice(&merged_bytes).unwrap();
        assert_eq!(merged.counter.value().unwrap(), 5); // 3 + 2
    }

    #[test]
    fn test_merge_by_type_name_unknown_type() {
        env::reset_for_testing();
        clear_merge_registry();

        let bytes = vec![1, 2, 3];

        // Unknown type should return None
        let result = try_merge_by_type_name("UnknownType", &bytes, &bytes, 100, 200);
        assert!(result.is_none());
    }
}
