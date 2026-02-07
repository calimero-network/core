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

use crate::collections::crdt_meta::{CrdtType, InnerType, MergeError, Mergeable};
use crate::collections::{Counter, LwwRegister, ReplicatedGrowableArray};
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
        // Types with known inner type info - can merge at byte level
        CrdtType::LwwRegister { inner } => merge_lww_register(existing, incoming, inner),
        CrdtType::Counter => merge_pn_counter(existing, incoming),
        CrdtType::GCounter => merge_g_counter(existing, incoming),
        CrdtType::Rga => merge_rga(existing, incoming),

        // Collections with nested generics - need registry path
        // (e.g., UnorderedMap<String, LwwRegister<u64>> has nested types)
        CrdtType::UnorderedMap | CrdtType::UnorderedSet | CrdtType::Vector => {
            Err(MergeError::WasmRequired {
                type_name: format!("{:?}", crdt_type),
            })
        }

        // Generic wrappers - need concrete T to deserialize
        CrdtType::UserStorage | CrdtType::FrozenStorage => Err(MergeError::WasmRequired {
            type_name: format!("{:?}", crdt_type),
        }),

        // App-level types
        CrdtType::Record => Err(MergeError::WasmRequired {
            type_name: "Record".to_owned(),
        }),
        CrdtType::Custom(type_name) => Err(MergeError::WasmRequired {
            type_name: type_name.clone(),
        }),
    }
}

/// Merge two LWW registers - later timestamp wins.
///
/// Dispatches to the appropriate typed merge function based on `InnerType`.
fn merge_lww_register(
    existing: &[u8],
    incoming: &[u8],
    inner: &InnerType,
) -> Result<Vec<u8>, MergeError> {
    match inner {
        InnerType::U8 => merge_lww_register_typed::<u8>(existing, incoming),
        InnerType::U16 => merge_lww_register_typed::<u16>(existing, incoming),
        InnerType::U32 => merge_lww_register_typed::<u32>(existing, incoming),
        InnerType::U64 => merge_lww_register_typed::<u64>(existing, incoming),
        InnerType::U128 => merge_lww_register_typed::<u128>(existing, incoming),
        InnerType::I8 => merge_lww_register_typed::<i8>(existing, incoming),
        InnerType::I16 => merge_lww_register_typed::<i16>(existing, incoming),
        InnerType::I32 => merge_lww_register_typed::<i32>(existing, incoming),
        InnerType::I64 => merge_lww_register_typed::<i64>(existing, incoming),
        InnerType::I128 => merge_lww_register_typed::<i128>(existing, incoming),
        InnerType::F32 => merge_lww_register_typed::<f32>(existing, incoming),
        InnerType::F64 => merge_lww_register_typed::<f64>(existing, incoming),
        InnerType::Bool => merge_lww_register_typed::<bool>(existing, incoming),
        InnerType::String => merge_lww_register_typed::<String>(existing, incoming),
        InnerType::Bytes => merge_lww_register_typed::<Vec<u8>>(existing, incoming),
        InnerType::Custom(type_name) => Err(MergeError::WasmRequired {
            type_name: type_name.clone(),
        }),
    }
}

/// Type-safe merge for LwwRegister<T>
fn merge_lww_register_typed<T>(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError>
where
    T: borsh::BorshDeserialize + borsh::BorshSerialize + Clone,
{
    let mut existing_reg: LwwRegister<T> =
        borsh::from_slice(existing).map_err(|e| MergeError::SerializationError(e.to_string()))?;
    let incoming_reg: LwwRegister<T> =
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

// Note: UnorderedMap, UnorderedSet, and Vector have nested generic types
// (e.g., UnorderedMap<K, V>) that make byte-level merging unsafe without
// knowing the concrete types. They go through the registry path instead,
// where the app has registered the full type with its Mergeable impl.

/// Check if a CRDT type can be merged in the storage layer without knowing the concrete type.
///
/// Returns `true` for built-in types that have storage-layer merge implementations
/// and don't require unknown generic type parameters to deserialize.
///
/// **Builtin types** (can merge at byte level):
/// - `LwwRegister { inner }` - if inner is a known primitive type
/// - `Counter` (PNCounter), `GCounter` - no generics, internal maps use known types
/// - `Rga` - no generics, uses chars
///
/// **Registry types** (need app-registered concrete types):
/// - `UnorderedMap`, `UnorderedSet`, `Vector` - nested generics too complex
/// - `UserStorage`, `FrozenStorage` - generic wrappers
/// - `Record` - composite app state
/// - `Custom` - app-defined types
pub fn is_builtin_crdt(crdt_type: &CrdtType) -> bool {
    match crdt_type {
        // LwwRegister with known inner type can be merged
        CrdtType::LwwRegister { inner } => !matches!(inner, InnerType::Custom(_)),
        // Counters and RGA have no unknown generics
        CrdtType::Counter | CrdtType::GCounter | CrdtType::Rga => true,
        // Collections with nested generics go through registry
        CrdtType::UnorderedMap | CrdtType::UnorderedSet | CrdtType::Vector => false,
        // Generic wrappers and app types go through registry
        CrdtType::UserStorage
        | CrdtType::FrozenStorage
        | CrdtType::Record
        | CrdtType::Custom(_) => false,
    }
}
