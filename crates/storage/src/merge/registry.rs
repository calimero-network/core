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
#[derive(Clone)]
struct MergeEntry {
    merge_fn: MergeFn,
}

/// Injectable merge registry for CRDT types.
///
/// This struct holds registered merge functions and can be created fresh
/// for each test, avoiding global state issues.
#[derive(Default)]
pub struct MergeRegistry {
    by_type_id: HashMap<TypeId, MergeEntry>,
    by_type_name: HashMap<String, MergeFn>,
}

impl MergeRegistry {
    /// Creates a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a CRDT merge function for a type.
    ///
    /// This registers the merge function both by `TypeId` (for in-process dispatch)
    /// and by type name (for `CrdtType::Custom { type_name }` dispatch).
    pub fn register<T>(&mut self)
    where
        T: borsh::BorshSerialize
            + borsh::BorshDeserialize
            + crate::collections::Mergeable
            + 'static,
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
            let mut existing_state = borsh::from_slice::<T>(existing)
                .map_err(|e| format!("Failed to deserialize existing state: {}", e))?;

            let incoming_state = borsh::from_slice::<T>(incoming)
                .map_err(|e| format!("Failed to deserialize incoming state: {}", e))?;

            existing_state
                .merge(&incoming_state)
                .map_err(|e| format!("Merge failed: {}", e))?;

            borsh::to_vec(&existing_state)
                .map_err(|e| format!("Serialization failed: {}", e).into())
        };

        self.by_type_id.insert(type_id, MergeEntry { merge_fn });
        self.by_type_name.insert(simple_name, merge_fn);
    }

    /// Try to merge using registered merge function (brute force).
    ///
    /// Tries each registered function until one succeeds.
    /// For type-name-based dispatch (more efficient), use `try_merge_by_type_name`.
    pub fn try_merge(
        &self,
        existing: &[u8],
        incoming: &[u8],
        existing_ts: u64,
        incoming_ts: u64,
    ) -> Option<Result<Vec<u8>, Box<dyn std::error::Error>>> {
        for entry in self.by_type_id.values() {
            if let Ok(merged) = (entry.merge_fn)(existing, incoming, existing_ts, incoming_ts) {
                return Some(Ok(merged));
            }
        }
        None
    }

    /// Try to merge using type name (for CrdtType::Custom dispatch).
    ///
    /// This is more efficient than `try_merge` because it looks up
    /// directly by type name instead of trying all registered functions.
    pub fn try_merge_by_type_name(
        &self,
        type_name: &str,
        existing: &[u8],
        incoming: &[u8],
        existing_ts: u64,
        incoming_ts: u64,
    ) -> Option<Result<Vec<u8>, Box<dyn std::error::Error>>> {
        self.by_type_name
            .get(type_name)
            .map(|merge_fn| merge_fn(existing, incoming, existing_ts, incoming_ts))
    }

    /// Check if a type name is registered.
    #[must_use]
    pub fn contains_type_name(&self, type_name: &str) -> bool {
        self.by_type_name.contains_key(type_name)
    }

    /// Clear all registrations.
    pub fn clear(&mut self) {
        self.by_type_id.clear();
        self.by_type_name.clear();
    }
}

// =============================================================================
// Global registry (for backward compatibility in production)
// =============================================================================

/// Global registry of merge functions by TypeId
static MERGE_REGISTRY: LazyLock<RwLock<HashMap<TypeId, MergeEntry>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Global registry of merge functions by type name (for CrdtType::Custom dispatch)
static NAME_REGISTRY: LazyLock<RwLock<HashMap<String, MergeFn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a CRDT merge function for a type (global registry).
///
/// For tests, prefer using `MergeRegistry::new()` and `registry.register::<T>()`.
pub fn register_crdt_merge<T>()
where
    T: borsh::BorshSerialize + borsh::BorshDeserialize + crate::collections::Mergeable + 'static,
{
    let type_id = TypeId::of::<T>();
    let type_name = std::any::type_name::<T>().to_owned();

    let simple_name = type_name
        .rsplit("::")
        .next()
        .unwrap_or(&type_name)
        .to_owned();

    let merge_fn: MergeFn = |existing, incoming, _existing_ts, _incoming_ts| {
        let mut existing_state = borsh::from_slice::<T>(existing)
            .map_err(|e| format!("Failed to deserialize existing state: {}", e))?;

        let incoming_state = borsh::from_slice::<T>(incoming)
            .map_err(|e| format!("Failed to deserialize incoming state: {}", e))?;

        existing_state
            .merge(&incoming_state)
            .map_err(|e| format!("Merge failed: {}", e))?;

        borsh::to_vec(&existing_state).map_err(|e| format!("Serialization failed: {}", e).into())
    };

    {
        let mut registry = MERGE_REGISTRY
            .write()
            .unwrap_or_else(|_| std::process::abort());
        let _ = registry.insert(type_id, MergeEntry { merge_fn });
    }

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

/// Try to merge using registered merge function (brute force) - global registry.
pub fn try_merge_registered(
    existing: &[u8],
    incoming: &[u8],
    existing_ts: u64,
    incoming_ts: u64,
) -> Option<Result<Vec<u8>, Box<dyn std::error::Error>>> {
    let registry = MERGE_REGISTRY.read().ok()?;

    tracing::debug!(
        target: "storage::merge",
        registered_types = registry.len(),
        "Trying registered merge functions"
    );

    for entry in registry.values() {
        match (entry.merge_fn)(existing, incoming, existing_ts, incoming_ts) {
            Ok(merged) => {
                tracing::info!(
                    target: "storage::merge",
                    merged_len = merged.len(),
                    "Successfully merged using registered function"
                );
                return Some(Ok(merged));
            }
            Err(e) => {
                tracing::trace!(
                    target: "storage::merge",
                    error = %e,
                    "Merge function failed, trying next"
                );
            }
        }
    }

    tracing::debug!(
        target: "storage::merge",
        "No registered merge function succeeded"
    );

    None
}

/// Try to merge using type name (for CrdtType::Custom dispatch) - global registry.
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
    use crate::collections::Mergeable;

    // =========================================================================
    // PURE test types - NO storage operations!
    // =========================================================================

    /// Simple counter that doesn't touch storage - just adds values
    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize, Debug, Clone, PartialEq)]
    struct PureCounter {
        value: i64,
    }

    impl PureCounter {
        fn new(value: i64) -> Self {
            Self { value }
        }
    }

    impl Mergeable for PureCounter {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            // G-Counter semantics: sum the values
            self.value += other.value;
            Ok(())
        }
    }

    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize, Debug, Clone, PartialEq)]
    struct TestState {
        counter: PureCounter,
    }

    impl Mergeable for TestState {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            self.counter.merge(&other.counter)
        }
    }

    // =========================================================================
    // Tests using injectable MergeRegistry (preferred - fully isolated)
    // =========================================================================

    #[test]
    fn test_injectable_registry_merge() {
        let mut registry = MergeRegistry::new();
        registry.register::<TestState>();

        let state1 = TestState {
            counter: PureCounter::new(2),
        };
        let state2 = TestState {
            counter: PureCounter::new(1),
        };

        let bytes1 = borsh::to_vec(&state1).unwrap();
        let bytes2 = borsh::to_vec(&state2).unwrap();

        let merged_bytes = registry
            .try_merge(&bytes1, &bytes2, 100, 200)
            .unwrap()
            .unwrap();

        let merged: TestState = borsh::from_slice(&merged_bytes).unwrap();
        assert_eq!(merged.counter.value, 3); // 2 + 1
    }

    #[test]
    fn test_injectable_registry_by_type_name() {
        let mut registry = MergeRegistry::new();
        registry.register::<TestState>();

        let state1 = TestState {
            counter: PureCounter::new(3),
        };
        let state2 = TestState {
            counter: PureCounter::new(2),
        };

        let bytes1 = borsh::to_vec(&state1).unwrap();
        let bytes2 = borsh::to_vec(&state2).unwrap();

        let merged_bytes = registry
            .try_merge_by_type_name("TestState", &bytes1, &bytes2, 100, 200)
            .expect("Should find registered type")
            .expect("Merge should succeed");

        let merged: TestState = borsh::from_slice(&merged_bytes).unwrap();
        assert_eq!(merged.counter.value, 5); // 3 + 2
    }

    #[test]
    fn test_injectable_registry_unknown_type() {
        let registry = MergeRegistry::new();
        let bytes = vec![1, 2, 3];

        let result = registry.try_merge_by_type_name("UnknownType", &bytes, &bytes, 100, 200);
        assert!(result.is_none());
    }

    #[test]
    fn test_injectable_registry_contains() {
        let mut registry = MergeRegistry::new();
        assert!(!registry.contains_type_name("TestState"));

        registry.register::<TestState>();
        assert!(registry.contains_type_name("TestState"));

        registry.clear();
        assert!(!registry.contains_type_name("TestState"));
    }

    // =========================================================================
    // Tests using global registry (backward compatibility)
    // =========================================================================

    #[test]
    fn test_global_register_and_merge() {
        clear_merge_registry();
        register_crdt_merge::<TestState>();

        let state1 = TestState {
            counter: PureCounter::new(2),
        };
        let state2 = TestState {
            counter: PureCounter::new(1),
        };

        let bytes1 = borsh::to_vec(&state1).unwrap();
        let bytes2 = borsh::to_vec(&state2).unwrap();

        let merged_bytes = try_merge_registered(&bytes1, &bytes2, 100, 200)
            .unwrap()
            .unwrap();

        let merged: TestState = borsh::from_slice(&merged_bytes).unwrap();
        assert_eq!(merged.counter.value, 3);
    }

    #[test]
    fn test_global_merge_by_type_name() {
        clear_merge_registry();
        register_crdt_merge::<TestState>();

        let state1 = TestState {
            counter: PureCounter::new(3),
        };
        let state2 = TestState {
            counter: PureCounter::new(2),
        };

        let bytes1 = borsh::to_vec(&state1).unwrap();
        let bytes2 = borsh::to_vec(&state2).unwrap();

        let merged_bytes = try_merge_by_type_name("TestState", &bytes1, &bytes2, 100, 200)
            .expect("Should find registered type")
            .expect("Merge should succeed");

        let merged: TestState = borsh::from_slice(&merged_bytes).unwrap();
        assert_eq!(merged.counter.value, 5);
    }

    #[test]
    fn test_global_merge_unknown_type() {
        clear_merge_registry();

        let bytes = vec![1, 2, 3];
        let result = try_merge_by_type_name("UnknownType", &bytes, &bytes, 100, 200);
        assert!(result.is_none());
    }
}
