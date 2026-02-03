//! CRDT merge logic for concurrent updates.
//!
//! This module implements merge strategies for resolving conflicts when
//! multiple nodes update the same data concurrently.

pub mod registry;
pub use registry::{register_crdt_merge, try_merge_by_type_name, try_merge_registered};

#[cfg(test)]
pub use registry::clear_merge_registry;

use borsh::{BorshDeserialize, BorshSerialize};

/// Merges root state as a Record CRDT.
///
/// Root is a Record CRDT that merges field-by-field using each field's merge function.
/// This is automatically handled by the registered merge function (from #[app::state] macro),
/// which calls Mergeable::merge() that recursively merges all CRDT fields.
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
/// 1. **Try registered merge:** Uses the merge function registered via `register_crdt_merge()`
///    - This function deserializes both states
///    - Calls `Mergeable::merge()` which merges field-by-field
///    - Each field's merge function is called recursively (UserStorage, FrozenStorage, etc.)
/// 2. **Error if not registered:** Root MUST have a registered merge function
///
/// # Why Record?
///
/// Root is conceptually a Record CRDT type - it's a struct/record that contains
/// multiple CRDT fields. The Record merges by calling each field's merge function,
/// which is exactly what the auto-generated Mergeable implementation does.
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
/// Returns error if merge fails (root requires registered merge function)
pub fn merge_root_state(
    existing: &[u8],
    incoming: &[u8],
    existing_ts: u64,
    incoming_ts: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Root is a Record CRDT - it merges field-by-field using children's merge functions
    // The registered merge function (from #[app::state] macro) implements this:
    // 1. Deserializes both states
    // 2. Calls Mergeable::merge() which merges each CRDT field
    // 3. Each field's merge function is called recursively (UserStorage, FrozenStorage, etc.)
    match try_merge_registered(existing, incoming, existing_ts, incoming_ts) {
        Some(Ok(merged)) => Ok(merged),
        Some(Err(e)) => {
            // Merge function was registered but failed (e.g., deserialization error)
            Err(format!("Root state merge failed: {}. Root state is a Record CRDT that merges using its children's merge functions. Apps using #[app::state] must call register_crdt_merge() (auto-generated as __calimero_register_merge).", e).into())
        }
        None => {
            // No registered merge function found
            Err("Root state is a Record CRDT that merges using its children's merge functions. Apps using #[app::state] must call register_crdt_merge() (auto-generated as __calimero_register_merge).".into())
        }
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

/// Trait for WASM merge callbacks used during state synchronization.
///
/// This trait allows the runtime layer to provide custom merge logic
/// for `CrdtType::Custom` types via WASM callbacks.
///
/// # Example
///
/// ```ignore
/// // In runtime layer:
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
    ///
    /// # Errors
    /// Returns `WasmMergeError` if the WASM merge callback fails or the type is not registered.
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

/// A callback that uses the in-process merge registry (global).
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
