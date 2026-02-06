//! CRDT merge logic for concurrent updates.
//!
//! This module implements merge strategies for resolving conflicts when
//! multiple nodes update the same data concurrently.
//!
//! # Merge Dispatch
//!
//! When synchronizing state, the storage layer needs to merge concurrent updates.
//! The [`merge_by_crdt_type`] function dispatches to the correct merge implementation
//! based on the `CrdtType` stored in entity metadata:
//!
//! - **Built-in types** (Counter, Map, Set, etc.) - merged in storage layer
//! - **Custom types** - returns `WasmRequired` error for WASM callback
//! - **None** - falls back to LWW (Last-Write-Wins)

pub mod registry;
pub use registry::{register_crdt_merge, try_merge_registered};

#[cfg(test)]
pub use registry::clear_merge_registry;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::collections::crdt_meta::{CrdtType, MergeError, Mergeable};
use crate::collections::{
    Counter, LwwRegister, ReplicatedGrowableArray, UnorderedMap, UnorderedSet, Vector,
};
use crate::store::MainStorage;

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

/// Merge two Borsh-serialized values based on their CRDT type.
///
/// This function dispatches to the correct merge implementation based on the
/// `CrdtType` stored in entity metadata, enabling proper CRDT merge semantics
/// during synchronization.
///
/// # CIP Invariants
///
/// - **I5 (No Silent Data Loss)**: Built-in CRDT types are merged using their
///   semantic rules (e.g., Counter sums, Set unions), not overwritten.
/// - **I10 (Metadata Persistence)**: Relies on `crdt_type` being persisted in
///   entity metadata for correct dispatch.
///
/// # Arguments
///
/// * `crdt_type` - The CRDT type from entity metadata
/// * `existing` - Currently stored value (Borsh-serialized)
/// * `incoming` - Incoming value to merge (Borsh-serialized)
///
/// # Returns
///
/// Merged value as Borsh-serialized bytes.
///
/// # Errors
///
/// - `MergeError::WasmRequired` - For `Custom` types that need WASM callback
/// - `MergeError::SerializationError` - If deserialization/serialization fails
/// - `MergeError::StorageError` - If the underlying merge operation fails
pub fn merge_by_crdt_type(
    crdt_type: &CrdtType,
    existing: &[u8],
    incoming: &[u8],
) -> Result<Vec<u8>, MergeError> {
    match crdt_type {
        CrdtType::LwwRegister => merge_lww_register(existing, incoming),
        CrdtType::Counter => merge_pn_counter(existing, incoming),
        CrdtType::GCounter => merge_g_counter(existing, incoming),
        CrdtType::Rga => merge_rga(existing, incoming),
        CrdtType::UnorderedMap => merge_unordered_map(existing, incoming),
        CrdtType::UnorderedSet => merge_unordered_set(existing, incoming),
        CrdtType::Vector => merge_vector(existing, incoming),
        CrdtType::UserStorage | CrdtType::FrozenStorage => {
            // These are generic wrappers (UserStorage<T>, FrozenStorage<T>)
            // that require knowing the concrete type T to deserialize.
            // They should go through the registry path where the app
            // has registered the concrete type's Mergeable impl.
            Err(MergeError::WasmRequired {
                type_name: format!("{:?}", crdt_type),
            })
        }
        CrdtType::Record => {
            // Record types should be handled by the app-level merge callback
            // as they contain multiple fields with different CRDT types
            Err(MergeError::WasmRequired {
                type_name: "Record".to_owned(),
            })
        }
        CrdtType::Custom(type_name) => Err(MergeError::WasmRequired {
            type_name: type_name.clone(),
        }),
    }
}

/// Merge two LWW registers - later timestamp wins.
///
/// LwwRegister stores `(timestamp, value)`, so we compare timestamps
/// and keep the one with the higher timestamp.
fn merge_lww_register(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    // LwwRegister<T> serializes as (timestamp: u64, value: T)
    // We can deserialize with any T that implements BorshDeserialize
    // For now, use Vec<u8> as a generic wrapper to preserve the value bytes

    let mut existing_reg: LwwRegister<Vec<u8>> =
        borsh::from_slice(existing).map_err(|e| MergeError::SerializationError(e.to_string()))?;
    let incoming_reg: LwwRegister<Vec<u8>> =
        borsh::from_slice(incoming).map_err(|e| MergeError::SerializationError(e.to_string()))?;

    // Use the Mergeable impl which compares timestamps
    Mergeable::merge(&mut existing_reg, &incoming_reg)?;

    borsh::to_vec(&existing_reg).map_err(|e| MergeError::SerializationError(e.to_string()))
}

/// Merge two G-Counters (grow-only counters).
///
/// G-Counter merge takes the max count per executor.
fn merge_g_counter(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    let mut existing_counter: Counter<false, MainStorage> =
        borsh::from_slice(existing).map_err(|e| MergeError::SerializationError(e.to_string()))?;
    let incoming_counter: Counter<false, MainStorage> =
        borsh::from_slice(incoming).map_err(|e| MergeError::SerializationError(e.to_string()))?;

    Mergeable::merge(&mut existing_counter, &incoming_counter)?;

    borsh::to_vec(&existing_counter).map_err(|e| MergeError::SerializationError(e.to_string()))
}

/// Merge two PN-Counters (positive-negative counters).
///
/// PN-Counter merge takes the max count per executor for both positive and negative maps.
fn merge_pn_counter(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    let mut existing_counter: Counter<true, MainStorage> =
        borsh::from_slice(existing).map_err(|e| MergeError::SerializationError(e.to_string()))?;
    let incoming_counter: Counter<true, MainStorage> =
        borsh::from_slice(incoming).map_err(|e| MergeError::SerializationError(e.to_string()))?;

    Mergeable::merge(&mut existing_counter, &incoming_counter)?;

    borsh::to_vec(&existing_counter).map_err(|e| MergeError::SerializationError(e.to_string()))
}

/// Merge two RGAs (Replicated Growable Arrays).
///
/// RGA merges by unioning all characters from both arrays,
/// with ordering determined by (timestamp, node_id).
fn merge_rga(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    let mut existing_rga: ReplicatedGrowableArray =
        borsh::from_slice(existing).map_err(|e| MergeError::SerializationError(e.to_string()))?;
    let incoming_rga: ReplicatedGrowableArray =
        borsh::from_slice(incoming).map_err(|e| MergeError::SerializationError(e.to_string()))?;

    Mergeable::merge(&mut existing_rga, &incoming_rga)?;

    borsh::to_vec(&existing_rga).map_err(|e| MergeError::SerializationError(e.to_string()))
}

/// Merge two UnorderedMaps.
///
/// Map merges by unioning keys and recursively merging values
/// for keys that exist in both maps.
fn merge_unordered_map(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    // UnorderedMap<K, V> requires V: Mergeable for the Mergeable impl
    // For byte-level merging, we use LwwRegister<Vec<u8>> as the value type
    // which enables timestamp-based conflict resolution for nested values
    let mut existing_map: UnorderedMap<Vec<u8>, LwwRegister<Vec<u8>>, MainStorage> =
        borsh::from_slice(existing).map_err(|e| MergeError::SerializationError(e.to_string()))?;
    let incoming_map: UnorderedMap<Vec<u8>, LwwRegister<Vec<u8>>, MainStorage> =
        borsh::from_slice(incoming).map_err(|e| MergeError::SerializationError(e.to_string()))?;

    Mergeable::merge(&mut existing_map, &incoming_map)?;

    borsh::to_vec(&existing_map).map_err(|e| MergeError::SerializationError(e.to_string()))
}

/// Merge two UnorderedSets.
///
/// Set merges using union (add-wins) semantics - all elements
/// from both sets are preserved.
fn merge_unordered_set(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    let mut existing_set: UnorderedSet<Vec<u8>, MainStorage> =
        borsh::from_slice(existing).map_err(|e| MergeError::SerializationError(e.to_string()))?;
    let incoming_set: UnorderedSet<Vec<u8>, MainStorage> =
        borsh::from_slice(incoming).map_err(|e| MergeError::SerializationError(e.to_string()))?;

    Mergeable::merge(&mut existing_set, &incoming_set)?;

    borsh::to_vec(&existing_set).map_err(|e| MergeError::SerializationError(e.to_string()))
}

/// Merge two Vectors.
///
/// Vector merges element-by-element at same indices,
/// with longer vector determining final length.
fn merge_vector(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    // Vector<T> requires T: Mergeable for the Mergeable impl
    // Use LwwRegister<Vec<u8>> for timestamp-based nested value merging
    let mut existing_vec: Vector<LwwRegister<Vec<u8>>, MainStorage> =
        borsh::from_slice(existing).map_err(|e| MergeError::SerializationError(e.to_string()))?;
    let incoming_vec: Vector<LwwRegister<Vec<u8>>, MainStorage> =
        borsh::from_slice(incoming).map_err(|e| MergeError::SerializationError(e.to_string()))?;

    Mergeable::merge(&mut existing_vec, &incoming_vec)?;

    borsh::to_vec(&existing_vec).map_err(|e| MergeError::SerializationError(e.to_string()))
}

/// Check if a CRDT type can be merged in the storage layer without knowing the concrete type.
///
/// Returns `true` for built-in types that have storage-layer merge implementations
/// and don't require generic type parameters to deserialize.
///
/// Returns `false` for:
/// - `UserStorage`/`FrozenStorage` - generic wrappers, need concrete T to deserialize
/// - `Record` - composite app state, uses registry
/// - `Custom` - app-defined types, need WASM callback
pub fn is_builtin_crdt(crdt_type: &CrdtType) -> bool {
    matches!(
        crdt_type,
        CrdtType::LwwRegister
            | CrdtType::Counter
            | CrdtType::GCounter
            | CrdtType::Rga
            | CrdtType::UnorderedMap
            | CrdtType::UnorderedSet
            | CrdtType::Vector
    )
}
