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
//! # Storage
//!
//! Production uses a process-global `RwLock<HashMap<...>>`; apps register
//! their state types once at startup and every async worker dispatches
//! against the same table.
//!
//! Under `#[cfg(test)]` the backing store is a `thread_local!` so that
//! parallel-running tests can't stomp on each other's registrations.
//! See the comment on the `#[cfg(test)]` declaration below.

use std::any::TypeId;
#[cfg(test)]
use std::cell::RefCell;
use std::collections::HashMap;
#[cfg(not(test))]
use std::sync::{LazyLock, RwLock};

/// Function signature for merging serialized state
pub type MergeFn = fn(&[u8], &[u8], u64, u64) -> Result<Vec<u8>, Box<dyn std::error::Error>>;

/// Production registry — process-global, shared across async workers.
#[cfg(not(test))]
static MERGE_REGISTRY: LazyLock<RwLock<HashMap<TypeId, MergeFn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

// Test registry — per-thread. cargo test runs tests in parallel on
// different threads; with a global registry, a test calling
// `clear_merge_registry()` (e.g. to assert "no merge functions
// registered" behaviour) could wipe entries that another thread's
// test had just populated via `register_test_merge_functions()`, and
// the subsequent `apply_action` on that other thread would then fail
// dispatch mid-flight. `#[serial]` only serialises the clearers
// against each other — unrelated non-serial tests still ran in
// parallel with them. Thread-local storage makes each test's
// registry state private to its own thread, so the race can't occur.
#[cfg(test)]
thread_local! {
    static MERGE_REGISTRY: RefCell<HashMap<TypeId, MergeFn>> = RefCell::new(HashMap::new());
}

/// Run `f` with mutable access to the registry.
#[cfg(not(test))]
fn with_registry_mut<R>(f: impl FnOnce(&mut HashMap<TypeId, MergeFn>) -> R) -> R {
    let mut registry = MERGE_REGISTRY.write().unwrap_or_else(|_| {
        tracing::error!(
            target: "calimero_storage::merge",
            "MERGE_REGISTRY lock poisoned during write, aborting. This indicates a panic in merge code."
        );
        std::process::abort()
    });
    f(&mut registry)
}

#[cfg(test)]
fn with_registry_mut<R>(f: impl FnOnce(&mut HashMap<TypeId, MergeFn>) -> R) -> R {
    MERGE_REGISTRY.with(|r| f(&mut r.borrow_mut()))
}

/// Run `f` with read-only access to the registry.
#[cfg(not(test))]
fn with_registry<R>(f: impl FnOnce(&HashMap<TypeId, MergeFn>) -> R) -> R {
    let registry = MERGE_REGISTRY.read().unwrap_or_else(|_| {
        tracing::error!(
            target: "calimero_storage::merge",
            "MERGE_REGISTRY lock poisoned, aborting. This indicates a panic in merge code."
        );
        std::process::abort()
    });
    f(&registry)
}

#[cfg(test)]
fn with_registry<R>(f: impl FnOnce(&HashMap<TypeId, MergeFn>) -> R) -> R {
    MERGE_REGISTRY.with(|r| f(&r.borrow()))
}

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
        // CRITICAL: Use merge mode to prevent timestamp generation during merge.
        // Without this, different nodes generate different timestamps, causing
        // hash divergence even when logical state is identical.
        crate::env::with_merge_mode(|| {
            existing_state
                .merge(&incoming_state)
                .map_err(|e| format!("Merge failed: {}", e))
        })?;

        // Serialize result
        borsh::to_vec(&existing_state).map_err(|e| format!("Serialization failed: {}", e).into())
    };

    with_registry_mut(|registry| {
        let _ = registry.insert(type_id, merge_fn);
    });
}

/// Clear the merge registry (for testing only)
#[cfg(test)]
pub fn clear_merge_registry() {
    with_registry_mut(|registry| registry.clear());
}

/// Result of attempting to merge using registered merge functions
#[derive(Debug)]
#[must_use]
pub enum MergeRegistryResult {
    /// A registered merge function succeeded
    Success(Vec<u8>),
    /// No merge functions are registered (I5 enforcement needed)
    NoFunctionsRegistered,
    /// Merge functions are registered but all failed (e.g., type mismatch)
    AllFunctionsFailed,
}

/// Try to merge using registered merge function
///
/// Returns:
/// - `Success(merged)` if a merge function succeeded
/// - `NoFunctionsRegistered` if no merge functions are registered (I5 violation)
/// - `AllFunctionsFailed` if merge functions exist but none could merge the data
pub fn try_merge_registered(
    existing: &[u8],
    incoming: &[u8],
    existing_ts: u64,
    incoming_ts: u64,
) -> MergeRegistryResult {
    // For now, we don't have type information at runtime.
    // TODO: Store type hints with root entity for O(1) dispatch (see issue #1993)

    // Try each registered merge function until one succeeds (O(n) where n = registered types)
    with_registry(|registry| {
        if registry.is_empty() {
            return MergeRegistryResult::NoFunctionsRegistered;
        }

        for (_type_id, merge_fn) in registry.iter() {
            if let Ok(merged) = merge_fn(existing, incoming, existing_ts, incoming_ts) {
                return MergeRegistryResult::Success(merged);
            }
        }

        MergeRegistryResult::AllFunctionsFailed
    })
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

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
    #[serial]
    fn test_register_and_merge() {
        env::reset_for_testing();
        clear_merge_registry(); // Clear any previous registrations to ensure clean test

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
        let merged_bytes = match try_merge_registered(&bytes1, &bytes2, 100, 200) {
            MergeRegistryResult::Success(bytes) => bytes,
            MergeRegistryResult::NoFunctionsRegistered => {
                panic!("Expected merge function to be registered")
            }
            MergeRegistryResult::AllFunctionsFailed => {
                panic!("Expected merge to succeed")
            }
        };

        // Deserialize result
        let merged: TestState = borsh::from_slice(&merged_bytes).unwrap();

        // Verify: counters summed (2 + 1 = 3)
        assert_eq!(merged.counter.value().unwrap(), 3);
    }

    #[test]
    #[serial]
    fn test_no_merge_function_registered_returns_error() {
        use crate::merge::merge_root_state;

        env::reset_for_testing();
        clear_merge_registry(); // Ensure registry is empty

        // Create some arbitrary data
        let data1 = vec![1, 2, 3, 4];
        let data2 = vec![5, 6, 7, 8];

        // Attempt merge with no registered functions
        let result = merge_root_state(&data1, &data2, 100, 200);

        // Should return NoMergeFunctionRegistered error (I5 enforcement)
        assert!(
            result.is_err(),
            "Expected error when no merge function is registered"
        );

        let err = result.unwrap_err();
        assert!(
            matches!(
                err,
                crate::collections::crdt_meta::MergeError::NoMergeFunctionRegistered
            ),
            "Expected NoMergeFunctionRegistered error, got: {:?}",
            err
        );
    }
}
