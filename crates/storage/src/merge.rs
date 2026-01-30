//! CRDT merge logic for concurrent updates.
//!
//! This module implements merge strategies for resolving conflicts when
//! multiple nodes update the same data concurrently.

pub mod registry;
pub use registry::{register_crdt_merge, try_merge_by_type_name, try_merge_registered};

#[cfg(test)]
pub use registry::clear_merge_registry;

use borsh::{BorshDeserialize, BorshSerialize};

/// Attempts to merge two Borsh-serialized app state blobs using CRDT semantics.
///
/// # When is This Called?
///
/// **ONLY during remote synchronization**, specifically:
/// 1. When receiving a remote delta that updates the ROOT entity
/// 2. When concurrent updates to root state occur (same timestamp)
/// 3. NOT on local operations (those are O(1) direct writes)
///
/// # Performance
///
/// - **Local operations:** O(1) - this function is NOT called
/// - **Remote sync (different entities):** O(1) - this function is NOT called
/// - **Remote sync (root conflict):** O(N) - this function IS called
///   - Where N = number of root-level fields
///   - Frequency: RARE (only on concurrent root modifications)
///   - Typically: N = 10-100 fields
///   - Network latency >> merge time
///
/// # Strategy
///
/// 1. **Try registered merge:** If app called `register_crdt_merge()`, use type-specific merge
/// 2. **Fallback to LWW:** If no registered merge, use Last-Write-Wins
///
/// # Arguments
/// * `existing` - The currently stored state (Borsh-serialized)
/// * `incoming` - The new state being synced (Borsh-serialized)
/// * `existing_ts` - Timestamp of existing state
/// * `incoming_ts` - Timestamp of incoming state
///
/// # Returns
/// Merged state as Borsh-serialized bytes
///
/// # Errors
/// Returns error if merge fails (falls back to LWW in that case)
pub fn merge_root_state(
    existing: &[u8],
    incoming: &[u8],
    existing_ts: u64,
    incoming_ts: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Try registered CRDT merge functions first
    // This enables automatic nested CRDT merging when apps use #[app::state]
    if let Some(result) = try_merge_registered(existing, incoming, existing_ts, incoming_ts) {
        return result;
    }

    // NOTE: We can't blindly deserialize without knowing the type.
    // The collections (UnorderedMap, Vector, Counter, etc.) already handle
    // CRDT merging through their own element IDs and storage mechanisms.
    //
    // For root entities, concurrent updates should be rare since most operations
    // target nested entities (RGA characters, Map entries, etc.) which have their
    // own IDs and merge independently.
    //
    // Fallback: use LWW if no registered merge function
    // This is safe for simple apps or backward compatibility
    if incoming_ts >= existing_ts {
        Ok(incoming.to_vec())
    } else {
        Ok(existing.to_vec())
    }
}

/// Trait for app state types that need custom CRDT merge.
///
/// Implement this on your app's root state type to enable proper
/// concurrent update resolution.
///
/// # Example
///
/// ```ignore
/// #[derive(BorshSerialize, BorshDeserialize)]
/// struct MyAppState {
///     counter: GCounter,
///     items: UnorderedMap<String, String>,
/// }
///
/// impl CrdtMerge for MyAppState {
///     fn crdt_merge(&mut self, other: &Self) {
///         // Merge G-Counter
///         self.counter.merge(&other.counter);
///         
///         // UnorderedMap uses LWW per-key (handled by storage layer)
///     }
/// }
/// ```
pub trait CrdtMerge: BorshSerialize + BorshDeserialize {
    /// Merge another instance into self using CRDT semantics.
    fn crdt_merge(&mut self, other: &Self);
}

// ════════════════════════════════════════════════════════════════════════════
// WASM Merge Callback
// ════════════════════════════════════════════════════════════════════════════

/// Error type for WASM merge operations.
#[derive(Debug)]
pub enum WasmMergeError {
    /// The type name is not recognized by the WASM module.
    UnknownType(String),
    /// The WASM merge function returned an error.
    MergeFailed(String),
    /// Failed to serialize/deserialize data for WASM boundary.
    SerializationError(String),
}

impl std::fmt::Display for WasmMergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownType(name) => write!(f, "Unknown type for WASM merge: {}", name),
            Self::MergeFailed(msg) => write!(f, "WASM merge failed: {}", msg),
            Self::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
        }
    }
}

impl std::error::Error for WasmMergeError {}

/// Callback trait for merging custom types via WASM.
///
/// This trait is implemented by the runtime layer to provide WASM merge
/// functionality during state synchronization. When the storage layer
/// encounters a `CrdtType::Custom { type_name }`, it calls this callback
/// to merge the data using the app's custom merge logic.
///
/// # Architecture
///
/// ```text
/// ┌─────────────────────────────────────────────────────────────────┐
/// │                     State Sync Flow                              │
/// ├─────────────────────────────────────────────────────────────────┤
/// │                                                                  │
/// │  compare_trees()                                                 │
/// │       │                                                          │
/// │       ▼                                                          │
/// │  CrdtType::Custom { type_name }                                  │
/// │       │                                                          │
/// │       ▼                                                          │
/// │  WasmMergeCallback::merge_custom(type_name, local, remote)      │
/// │       │                                                          │
/// │       ▼                                                          │
/// │  ┌────────────────────────────────────────┐                     │
/// │  │  WASM Runtime                           │                     │
/// │  │  ├── Lookup merge fn by type_name      │                     │
/// │  │  ├── Deserialize local + remote        │                     │
/// │  │  ├── Call Mergeable::merge()           │                     │
/// │  │  └── Serialize result                  │                     │
/// │  └────────────────────────────────────────┘                     │
/// │       │                                                          │
/// │       ▼                                                          │
/// │  Merged bytes returned to storage layer                         │
/// │                                                                  │
/// └─────────────────────────────────────────────────────────────────┘
/// ```
///
/// # Implementation Notes
///
/// The runtime layer should:
/// 1. Extract the WASM module for the current context
/// 2. Look up the merge function by type name
/// 3. Call into WASM with the serialized local and remote data
/// 4. Return the merged result
///
/// # Example
///
/// ```ignore
/// struct RuntimeMergeCallback {
///     wasm_module: WasmModule,
/// }
///
/// impl WasmMergeCallback for RuntimeMergeCallback {
///     fn merge_custom(
///         &self,
///         type_name: &str,
///         local_data: &[u8],
///         remote_data: &[u8],
///         local_ts: u64,
///         remote_ts: u64,
///     ) -> Result<Vec<u8>, WasmMergeError> {
///         // Call WASM merge function
///         self.wasm_module.call_merge(type_name, local_data, remote_data)
///     }
/// }
/// ```
pub trait WasmMergeCallback: Send + Sync {
    /// Merge two instances of a custom type using WASM merge logic.
    ///
    /// # Arguments
    /// * `type_name` - The name of the custom type (from `CrdtType::Custom`)
    /// * `local_data` - Borsh-serialized local data
    /// * `remote_data` - Borsh-serialized remote data
    /// * `local_ts` - Timestamp of local data
    /// * `remote_ts` - Timestamp of remote data
    ///
    /// # Returns
    /// Borsh-serialized merged result, or error if merge fails.
    fn merge_custom(
        &self,
        type_name: &str,
        local_data: &[u8],
        remote_data: &[u8],
        local_ts: u64,
        remote_ts: u64,
    ) -> Result<Vec<u8>, WasmMergeError>;
}

/// A no-op callback that falls back to LWW for custom types.
///
/// Used when no WASM callback is available (e.g., tests, non-WASM contexts).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopMergeCallback;

impl WasmMergeCallback for NoopMergeCallback {
    fn merge_custom(
        &self,
        _type_name: &str,
        local_data: &[u8],
        remote_data: &[u8],
        local_ts: u64,
        remote_ts: u64,
    ) -> Result<Vec<u8>, WasmMergeError> {
        // Fallback to LWW
        if remote_ts >= local_ts {
            Ok(remote_data.to_vec())
        } else {
            Ok(local_data.to_vec())
        }
    }
}

/// A callback that uses the in-process merge registry.
///
/// This is useful when the WASM module has already registered its merge
/// function via `register_crdt_merge`. The runtime calls this after WASM
/// initialization to use the registered merge functions.
///
/// # Example
///
/// ```ignore
/// // After WASM module loads and calls __calimero_register_merge:
/// let callback = RegistryMergeCallback;
///
/// // During sync:
/// compare_trees_with_callback(data, index, Some(&callback));
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct RegistryMergeCallback;

impl WasmMergeCallback for RegistryMergeCallback {
    fn merge_custom(
        &self,
        type_name: &str,
        local_data: &[u8],
        remote_data: &[u8],
        local_ts: u64,
        remote_ts: u64,
    ) -> Result<Vec<u8>, WasmMergeError> {
        match try_merge_by_type_name(type_name, local_data, remote_data, local_ts, remote_ts) {
            Some(Ok(merged)) => Ok(merged),
            Some(Err(e)) => Err(WasmMergeError::MergeFailed(e.to_string())),
            None => Err(WasmMergeError::UnknownType(type_name.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::{Counter, Mergeable};
    use crate::env;

    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize, Debug)]
    struct CallbackTestState {
        counter: Counter,
    }

    impl Mergeable for CallbackTestState {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            self.counter.merge(&other.counter)
        }
    }

    #[test]
    fn test_noop_callback_uses_lww() {
        let callback = NoopMergeCallback;

        let local = vec![1, 2, 3];
        let remote = vec![4, 5, 6];

        // Remote wins when remote_ts >= local_ts
        let result = callback
            .merge_custom("AnyType", &local, &remote, 100, 200)
            .unwrap();
        assert_eq!(result, remote);

        // Local wins when local_ts > remote_ts
        let result = callback
            .merge_custom("AnyType", &local, &remote, 200, 100)
            .unwrap();
        assert_eq!(result, local);
    }

    #[test]
    fn test_registry_callback_uses_registered_merge() {
        env::reset_for_testing();
        registry::clear_merge_registry();

        // Register the type
        register_crdt_merge::<CallbackTestState>();

        // Create two states
        env::set_executor_id([50; 32]);
        let mut state1 = CallbackTestState {
            counter: Counter::new(),
        };
        state1.counter.increment().unwrap();
        state1.counter.increment().unwrap(); // value = 2

        env::set_executor_id([60; 32]);
        let mut state2 = CallbackTestState {
            counter: Counter::new(),
        };
        state2.counter.increment().unwrap(); // value = 1

        let bytes1 = borsh::to_vec(&state1).unwrap();
        let bytes2 = borsh::to_vec(&state2).unwrap();

        // Merge via RegistryMergeCallback
        let callback = RegistryMergeCallback;
        let merged_bytes = callback
            .merge_custom("CallbackTestState", &bytes1, &bytes2, 100, 200)
            .expect("Merge should succeed");

        let merged: CallbackTestState = borsh::from_slice(&merged_bytes).unwrap();
        assert_eq!(merged.counter.value().unwrap(), 3); // 2 + 1
    }

    #[test]
    fn test_registry_callback_unknown_type() {
        env::reset_for_testing();
        registry::clear_merge_registry();

        let callback = RegistryMergeCallback;
        let bytes = vec![1, 2, 3];

        let result = callback.merge_custom("UnknownType", &bytes, &bytes, 100, 200);

        assert!(matches!(result, Err(WasmMergeError::UnknownType(_))));
    }
}
