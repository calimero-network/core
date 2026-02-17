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
//! - **Built-in types** (Counter, RGA) - merged in storage layer
//! - **Custom types** - returns `WasmRequired` error for WASM callback
//! - **LwwRegister** - returns `WasmRequired` (needs type info for deserialization)
//!
//! # Root Entity Merge
//!
//! The [`merge_root_state`] function handles root entity conflicts. It **requires**
//! a merge function to be registered via [`register_crdt_merge`]. If no merge
//! function is registered, it returns an error rather than silently falling back
//! to LWW (which would violate I5).
//!
//! To register a merge function:
//! - Use `#[app::state]` macro (recommended, auto-registers)
//! - Call `register_crdt_merge::<YourState>()` manually
//!
//! # CIP Invariants
//!
//! - **I5 (No Silent Data Loss)**: Built-in CRDT types are merged using their
//!   semantic rules (e.g., Counter sums, Set unions), not overwritten via LWW.
//!   Root entity merge requires explicit registration to prevent silent data loss.
//! - **I10 (Metadata Persistence)**: Relies on `crdt_type` being persisted in
//!   entity metadata for correct dispatch.

pub mod registry;
pub use registry::{register_crdt_merge, try_merge_registered, MergeRegistryResult};

#[cfg(test)]
pub use registry::clear_merge_registry;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::collections::crdt_meta::{CrdtType, MergeError, Mergeable};
use crate::collections::{Counter, ReplicatedGrowableArray};
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
/// Uses registered merge function (Mergeable trait) to perform type-aware CRDT merge.
/// If no merge function is registered, returns an error.
///
/// # CIP Invariants
///
/// - **I5 (No Silent Data Loss)**: This function enforces I5 by requiring explicit
///   merge function registration. Without registration, it fails loudly rather than
///   falling back to LWW (which would silently discard CRDT contributions).
///
/// # How to Fix "No merge function registered" Error
///
/// 1. **Recommended**: Use the `#[app::state]` macro on your root state type.
///    This auto-generates and registers the merge function.
///
/// 2. **Manual**: Call `register_crdt_merge::<YourState>()` where `YourState`
///    implements the `Mergeable` trait.
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
/// Returns error if:
/// - No merge function is registered for the root entity type (`MergeError::NoMergeFunctionRegistered`)
/// - The registered merge function fails
pub fn merge_root_state(
    existing: &[u8],
    incoming: &[u8],
    existing_ts: u64,
    incoming_ts: u64,
) -> Result<Vec<u8>, MergeError> {
    // Try registered CRDT merge functions first
    // This enables automatic nested CRDT merging when apps use #[app::state]
    match try_merge_registered(existing, incoming, existing_ts, incoming_ts) {
        MergeRegistryResult::Success(merged) => Ok(merged),
        MergeRegistryResult::NoFunctionsRegistered => {
            // I5 Enforcement: No silent data loss
            //
            // If the root entity contains CRDTs (Counter, etc.) and no merge function
            // is registered, an LWW fallback would cause DATA LOSS. One node's CRDT
            // contributions would be silently discarded.
            //
            // Instead of silently falling back to LWW, we fail with an actionable error
            // message telling the developer how to fix it.
            Err(MergeError::NoMergeFunctionRegistered)
        }
        MergeRegistryResult::AllFunctionsFailed => {
            // Merge functions are registered but none could merge the data.
            // This typically happens when:
            // - The data type doesn't match any registered type (test contamination)
            // - Deserialization failed (corrupt data)
            //
            // Fall back to LWW to maintain backwards compatibility.
            // The incoming value wins if timestamps are equal or incoming is newer.
            //
            // Per Delivery Contract Rule: any drop MUST be observable.
            tracing::warn!(
                target: "calimero_storage::merge",
                "All registered merge functions failed, falling back to LWW. \
                 This may indicate type mismatch or corrupt data."
            );
            if incoming_ts >= existing_ts {
                Ok(incoming.to_vec())
            } else {
                Ok(existing.to_vec())
            }
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

// =============================================================================
// CRDT Type-Based Merge Dispatch
// =============================================================================

/// Merge two Borsh-serialized values based on their CRDT type.
///
/// This function dispatches to the correct merge implementation based on the
/// `CrdtType` stored in entity metadata, enabling proper CRDT merge semantics
/// during synchronization.
///
/// # CIP Invariants
///
/// - **I5 (No Silent Data Loss)**: Built-in CRDT types are merged using their
///   semantic rules (e.g., GCounter takes max per executor), not overwritten.
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
/// - `MergeError::WasmRequired` - For types that need WASM callback
/// - `MergeError::SerializationError` - If deserialization/serialization fails
/// - `MergeError::StorageError` - If the underlying merge operation fails
///
/// # Example
///
/// ```ignore
/// use calimero_primitives::crdt::CrdtType;
/// use calimero_storage::merge::merge_by_crdt_type;
///
/// // During sync, when two nodes have concurrent GCounter updates:
/// let merged = merge_by_crdt_type(
///     &CrdtType::GCounter,
///     &existing_bytes,
///     &incoming_bytes,
/// )?;
/// ```
pub fn merge_by_crdt_type(
    crdt_type: &CrdtType,
    existing: &[u8],
    incoming: &[u8],
) -> Result<Vec<u8>, MergeError> {
    match crdt_type {
        // Counters - can merge at byte level
        CrdtType::GCounter => merge_g_counter(existing, incoming),
        CrdtType::PnCounter => merge_pn_counter(existing, incoming),

        // RGA - can merge at byte level
        CrdtType::Rga => merge_rga(existing, incoming),

        // LwwRegister - return incoming, caller handles timestamp comparison
        // The caller (try_merge_non_root) compares metadata HLC timestamps
        // and decides which value to keep based on that comparison.
        CrdtType::LwwRegister { .. } => Ok(incoming.to_vec()),

        // Collections - with type info we can merge them
        CrdtType::UnorderedMap { .. } => merge_unordered_map(existing, incoming),
        CrdtType::UnorderedSet { .. } => merge_unordered_set(existing, incoming),
        CrdtType::Vector { .. } => merge_vector(existing, incoming),

        // UserStorage - LWW per user (same as LwwRegister)
        CrdtType::UserStorage => Ok(incoming.to_vec()),

        // FrozenStorage - first-write-wins (keep existing)
        // Note: If two nodes independently write different first values before syncing,
        // they will each keep their own value (no convergence). This is by design for
        // immutable data like identity keys or genesis state where the first write is
        // authoritative. For data that must converge, use LwwRegister or UserStorage.
        CrdtType::FrozenStorage => Ok(existing.to_vec()),

        // App-defined types
        CrdtType::Custom(type_name) => Err(MergeError::WasmRequired {
            type_name: type_name.clone(),
        }),
    }
}

/// Check if a CRDT type can be merged in the storage layer without WASM callback.
///
/// Returns `true` for built-in types that have storage-layer merge implementations.
/// Only `Custom` types require WASM (app-defined merge logic).
///
/// **Builtin types**:
/// - `GCounter`, `PnCounter` - max per executor
/// - `Rga` - interleave by timestamp
/// - `LwwRegister` - LWW using metadata timestamps  
/// - `UnorderedMap`, `UnorderedSet`, `Vector` - structural merge
/// - `UserStorage` - LWW per user
/// - `FrozenStorage` - first-write-wins
///
/// **WASM types**:
/// - `Custom` - app-defined merge logic
///
/// # Example
///
/// ```ignore
/// use calimero_primitives::crdt::CrdtType;
/// use calimero_storage::merge::is_builtin_crdt;
///
/// assert!(is_builtin_crdt(&CrdtType::GCounter));
/// assert!(is_builtin_crdt(&CrdtType::UserStorage));
/// assert!(!is_builtin_crdt(&CrdtType::Custom("MyType".into())));
/// ```
pub fn is_builtin_crdt(crdt_type: &CrdtType) -> bool {
    !matches!(crdt_type, CrdtType::Custom(_))
}

// =============================================================================
// Type-Specific Merge Implementations
// =============================================================================

/// Merge two G-Counters (grow-only counters).
///
/// G-Counter merge takes the max count per executor. Each executor's increments
/// are tracked independently, and merge unions all executors taking max per executor.
///
/// # Arguments
///
/// * `existing` - Currently stored GCounter (Borsh-serialized)
/// * `incoming` - Incoming GCounter to merge (Borsh-serialized)
///
/// # Returns
///
/// Merged GCounter as Borsh-serialized bytes.
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
/// The final value is sum(positive) - sum(negative).
///
/// # Arguments
///
/// * `existing` - Currently stored PNCounter (Borsh-serialized)
/// * `incoming` - Incoming PNCounter to merge (Borsh-serialized)
///
/// # Returns
///
/// Merged PNCounter as Borsh-serialized bytes.
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
///
/// # Arguments
///
/// * `existing` - Currently stored RGA (Borsh-serialized)
/// * `incoming` - Incoming RGA to merge (Borsh-serialized)
///
/// # Returns
///
/// Merged RGA as Borsh-serialized bytes.
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
/// Collections use "Structured" storage where entries are stored as separate entities.
/// The container itself stores minimal metadata (ID, child references).
/// Actual entry merging happens when individual entries sync - here we just merge
/// the container by preferring incoming (add-wins semantics means entries accumulate).
fn merge_unordered_map(_existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    // For structured collections, entries are synced separately.
    // The container merge just ensures we have the latest structure.
    // Add-wins semantics: incoming may have new entries we don't know about.
    Ok(incoming.to_vec())
}

/// Merge two UnorderedSets.
///
/// Collections use "Structured" storage where elements are stored as separate entities.
/// Container merge prefers incoming (add-wins semantics).
fn merge_unordered_set(_existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    Ok(incoming.to_vec())
}

/// Merge two Vectors.
///
/// Collections use "Structured" storage where elements are stored as separate entities.
/// Container merge prefers incoming.
fn merge_vector(_existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    Ok(incoming.to_vec())
}
