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

pub mod custom_registry;
pub mod registry;

// The registry is WASM-only in production. Host production binaries
// can no longer call `register_crdt_merge` (it doesn't exist) or
// pattern-match on `MergeRegistryResult` (also gone). Host root-state
// merges route through `merge_root_state_typed` via the WASM
// `__calimero_merge_root_state` export +
// `ContextClient::merge_root_state` — see
// [`crate::merge::registry`] module docs for the rationale (core#2469).
//
// The `testing` feature flag re-exposes the registry to dependent
// crates' tests (calimero-storage integration tests, calimero-node
// sim tests) so they can keep exercising the WASM-side dispatch
// shape without spinning up a real WASM runtime.
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
pub use registry::{register_crdt_merge, try_merge_registered, MergeRegistryResult};

// Always available: both have a host fallback, so the registration walk and the
// merge dispatch compile from any build rather than only the ones that can act
// on them. See `custom_registry`.
pub use custom_registry::{
    custom_type_id_of, has_custom_merges, merge_custom, register_custom_merge,
};

// Always-native wrapper for the in-process test harness. Unlike
// `register_crdt_merge` it isn't gated behind the `testing` feature, so an
// app's macro-generated `TestState` bridge compiles under `cargo test`
// regardless of whether that app enabled the feature.
#[cfg(not(target_arch = "wasm32"))]
pub use registry::register_crdt_merge_for_test;

#[cfg(any(test, feature = "testing"))]
pub use registry::clear_merge_registry;

#[cfg(any(test, feature = "testing"))]
pub use custom_registry::clear_custom_merge_registry;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::collections::crdt_meta::{CrdtType, CustomTypeId, MergeError, Mergeable};
use crate::collections::{Counter, FugueText, ReplicatedGrowableArray, ROOT_ENTRY_ID};
use crate::store::MainStorage;

/// Canonical wire format for a host→WASM root-state merge invocation.
///
/// The host can't deserialize an app's root-state type (it doesn't have
/// the type at compile time), so when it needs to merge two root-state
/// byte blobs it sends this payload into the WASM module, where the
/// macro-generated `__calimero_merge_root_state` export knows the type
/// and dispatches `Mergeable::merge`.
///
/// Borsh-serialized for symmetry with every other host↔WASM payload in
/// the codebase.
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct MergeRootStateRequest {
    pub existing: Vec<u8>,
    pub incoming: Vec<u8>,
    pub existing_created_at: u64,
    pub existing_ts: u64,
    pub incoming_ts: u64,
}

/// Response from the WASM-side root-merge dispatcher.
///
/// `Ok(bytes)` carries the merged entry the host writes back into storage.
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub enum MergeRootStateResponse {
    Ok(Vec<u8>),
    /// A failure from a module that holds no merge of the stored entry, one built
    /// before `Refused` existed or a JS guest: the host applies its own rule.
    Err(String),
    /// The app's type does not read the incoming entry, or its merge failed:
    /// the stored entry stays, as a delta carrying it would be refused.
    Refused(String),
}

/// Request the host sends into WASM to merge one custom-typed entry. It carries no
/// timestamps, since an app rule that read them would not commute.
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct MergeCustomRequest {
    /// Which app-defined type this entry holds, as stamped on the entry.
    pub type_id: CustomTypeId,
    /// The receiver's stored entry bytes.
    pub existing: Vec<u8>,
    /// The entry bytes that arrived over the wire.
    pub incoming: Vec<u8>,
}

/// Response from the WASM-side custom-merge dispatcher.
///
/// `Err(message)` covers both a genuine merge failure and an id this build's
/// app no longer registers — an app-upgrade skew. Either way the host logs it
/// and leaves the entity for the next sync round rather than resolving it by
/// LWW, which would be the wrong answer for a conflict the app owns.
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub enum MergeCustomResponse {
    Ok(Vec<u8>),
    Err(String),
}

/// The app's merge of the app-state entry for a host request, run by the
/// `__calimero_merge_root_state` export: the rule a delta's write of the entry takes.
pub fn merge_root_state_typed<T>(request: &MergeRootStateRequest) -> MergeRootStateResponse
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
{
    let merged = merge_entry_values(
        &request.existing,
        &request.incoming,
        |existing, incoming| match merge_values::<T>(existing, incoming) {
            Err(MergeFnError::Existing) => Ok(incoming.to_vec()),
            merged => merged.map_err(|e| MergeError::SerializationError(format!("{e:?}"))),
        },
    );
    match merged {
        Ok(merged) => MergeRootStateResponse::Ok(merged),
        Err(e) => MergeRootStateResponse::Refused(e.to_string()),
    }
}

/// Why a pair of encoded values of one type was left unmerged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeFnError {
    /// The incoming value is not of this type.
    Incoming,
    /// The incoming value is of this type and the stored one is not.
    Existing,
    /// Both decode, but the merge or re-encoding failed.
    Merge,
}

/// Merges two encoded values of `T` under merge mode, so no node mints a stamp.
pub(crate) fn merge_values<T>(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeFnError>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
{
    let incoming_state = borsh::from_slice::<T>(incoming).map_err(|_| MergeFnError::Incoming)?;
    let mut existing_state =
        borsh::from_slice::<T>(existing).map_err(|_| MergeFnError::Existing)?;
    crate::env::with_merge_mode(|| existing_state.merge(&incoming_state))
        .map_err(|_| MergeFnError::Merge)?;
    borsh::to_vec(&existing_state).map_err(|_| MergeFnError::Merge)
}

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
/// - No registered type reads `incoming`, or the registered merge function fails
#[cfg_attr(
    not(any(target_arch = "wasm32", test, feature = "testing")),
    expect(
        unused_variables,
        reason = "without the registry nothing reads the stored value"
    )
)]
pub fn merge_root_state(
    existing: &[u8],
    incoming: &[u8],
    existing_created_at: u64,
    existing_ts: u64,
    incoming_ts: u64,
) -> Result<Vec<u8>, MergeError> {
    // Try registered CRDT merge functions first.
    // This enables automatic nested CRDT merging when apps use `#[app::state]`.
    //
    // NOTE: unlike `merge_root_state_typed`, the bootstrap fast-path is checked
    // only in the `NoFunctionsRegistered` arm below, NOT before dispatch. This
    // divergence is intentional and test-locked
    // (`test_root_cold_join_bootstrap_signal_with_merger_preserves_local_fields`):
    // when a CRDT merger IS registered it must run even on a cold-join bootstrap
    // signal, so local fields the joiner already materialised are preserved by
    // the merge rather than discarded by a verbatim accept-incoming.
    //
    // On host production builds the registry doesn't exist (deleted in
    // the WASM-owns-merges architectural fix for core#2469) — the local
    // closure below produces `NoFunctionsRegistered` directly so the
    // bootstrap fast-path / I5 error path stay reachable for the
    // (uncommon) host code paths that still call `merge_root_state`.
    // WASM and test builds still consult the real registry.
    #[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
    let dispatch_result = try_merge_registered(existing, incoming, existing_ts, incoming_ts);
    #[cfg(not(any(target_arch = "wasm32", test, feature = "testing")))]
    let dispatch_result = registry::MergeRegistryResult::NoFunctionsRegistered;
    match dispatch_result {
        registry::MergeRegistryResult::Success(merged) => Ok(merged),
        registry::MergeRegistryResult::NoFunctionsRegistered => {
            // Bootstrap-aware default.
            //
            // `existing_created_at == existing_ts` means the local entity
            // was created and has never been explicitly updated since —
            // the freshly-materialised default state on a joiner. In
            // that case the incoming side carries the only real history
            // and must be accepted unconditionally; plain LWW-by-HLC
            // would silently keep the local default because the
            // materialisation-time HLC is *later* than the remote's
            // earlier real write.
            if existing_created_at == existing_ts {
                tracing::debug!(
                    target: "calimero_storage::merge",
                    existing_created_at,
                    existing_ts,
                    incoming_ts,
                    "merge_root_state: bootstrap signal (created == updated, never written), accepting incoming"
                );
                return Ok(incoming.to_vec());
            }

            // I5 Enforcement: No silent data loss
            //
            // Both sides have real history, but no merger is registered.
            // An LWW fallback would silently drop one side's CRDT
            // contributions. Fail loudly instead with an actionable
            // error pointing the developer at `#[app::state]`.
            Err(MergeError::NoMergeFunctionRegistered)
        }
        // Bytes the app's own type does not read are not its state, so they
        // must never replace state it does read.
        registry::MergeRegistryResult::AllFunctionsFailed => Err(MergeError::SerializationError(
            "no registered root-state type reads the incoming value".to_owned(),
        )),
    }
}

/// Whether this build holds the app's root-state merger: a Rust app inside WASM.
pub(crate) fn has_root_merger() -> bool {
    #[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
    return registry::has_merge_functions();
    #[cfg(not(any(target_arch = "wasm32", test, feature = "testing")))]
    return false;
}

/// Merges two stored `Root<T>` entries, each `borsh(T)` followed by the entry id,
/// by running the registered merger over the values and putting the id back.
///
/// # Errors
///
/// As [`merge_entry_values`].
pub(crate) fn merge_root_entry(
    existing: &[u8],
    incoming: &[u8],
    existing_created_at: u64,
    existing_ts: u64,
    incoming_ts: u64,
) -> Result<Vec<u8>, MergeError> {
    merge_entry_values(existing, incoming, |existing, incoming| {
        merge_root_state(
            existing,
            incoming,
            existing_created_at,
            existing_ts,
            incoming_ts,
        )
    })
}

/// Merges two `Root<T>` entries by merging their values with `merge` and putting the id back.
///
/// # Errors
///
/// Refuses an incoming entry that does not end in its id, and what `merge` refuses.
/// A stored entry without the id reaches `merge` as an empty, unreadable value.
fn merge_entry_values(
    existing: &[u8],
    incoming: &[u8],
    merge: impl FnOnce(&[u8], &[u8]) -> Result<Vec<u8>, MergeError>,
) -> Result<Vec<u8>, MergeError> {
    let id = ROOT_ENTRY_ID.as_bytes().as_slice();
    let incoming_value = incoming.strip_suffix(id).ok_or_else(|| {
        MergeError::SerializationError("the incoming root entry does not end in its id".to_owned())
    })?;
    // A value merged with itself is unchanged, so an identical one is only read,
    // sparing a root's collections a walk of every entry they hold.
    let existing_value = existing
        .strip_suffix(id)
        .filter(|value| *value != incoming_value)
        .unwrap_or_default();

    let mut merged = merge(existing_value, incoming_value)?;
    merged.extend_from_slice(id);
    Ok(merged)
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
        CrdtType::UnorderedMap => merge_unordered_map(existing, incoming),
        // SortedMap stores and merges exactly like UnorderedMap (entries sync
        // separately; ordering is a read-time concern derived from `K: Ord`), so
        // the container merge is the same add-wins structural pass.
        CrdtType::SortedMap => merge_unordered_map(existing, incoming),
        CrdtType::UnorderedSet => merge_unordered_set(existing, incoming),
        // SortedSet stores/merges exactly like UnorderedSet (union; ordering is a
        // read-time concern derived from `T: Ord`).
        CrdtType::SortedSet => merge_unordered_set(existing, incoming),
        CrdtType::Vector => merge_vector(existing, incoming),

        // UserStorage - LWW per user (same as LwwRegister)
        CrdtType::UserStorage => Ok(incoming.to_vec()),

        // FrozenStorage - first-write-wins (keep existing)
        // Note: If two nodes independently write different first values before syncing,
        // they will each keep their own value (no convergence). This is by design for
        // immutable data like identity keys or genesis state where the first write is
        // authoritative. For data that must converge, use LwwRegister or UserStorage.
        CrdtType::FrozenStorage => Ok(existing.to_vec()),

        // SharedStorage - LWW per writer (same shape as UserStorage; per-writer
        // signature verification gates which deltas reach this point).
        CrdtType::SharedStorage => Ok(incoming.to_vec()),

        // RotationLog: decode-only legacy tag. `Interface` merges such a leaf by
        // LWW before it reaches here; this arm only completes the match.
        CrdtType::RotationLog => Ok(incoming.to_vec()),

        CrdtType::FugueText => merge_fugue_text(existing, incoming),
        CrdtType::FugueTextBlock => merge_fugue_text_block(existing, incoming),

        // App-defined types
        CrdtType::Custom(type_id) => Err(MergeError::WasmRequired { type_id: *type_id }),
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
/// - `UnorderedMap`, `SortedMap`, `UnorderedSet`, `SortedSet`, `Vector` - structural merge
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
/// # use calimero_primitives::crdt::CustomTypeId;
/// assert!(!is_builtin_crdt(&CrdtType::Custom(CustomTypeId::of("MyType"))));
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

fn merge_fugue_text(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    let mut existing_doc: FugueText =
        borsh::from_slice(existing).map_err(|e| MergeError::SerializationError(e.to_string()))?;
    let incoming_doc: FugueText =
        borsh::from_slice(incoming).map_err(|e| MergeError::SerializationError(e.to_string()))?;

    Mergeable::merge(&mut existing_doc, &incoming_doc)?;

    borsh::to_vec(&existing_doc).map_err(|e| MergeError::SerializationError(e.to_string()))
}

fn merge_fugue_text_block(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    FugueText::<MainStorage>::merge_block_entry_bytes(existing, incoming)
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

#[cfg(test)]
mod typed_dispatch_tests {
    use super::*;
    use crate::collections::{Counter, Root};
    use crate::env;
    use serial_test::serial;

    // Minimal Mergeable app type for the typed-dispatch test. Counter
    // is the simplest Mergeable that produces an observably-different
    // post-merge state from either input alone.
    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize, Debug)]
    struct DispatchTestApp {
        counter: Counter,
    }

    // RekeyTarget supertrait of Mergeable.
    impl crate::collections::rekey::RekeyTarget for DispatchTestApp {
        fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
            crate::rekey_field_if_supported!(
                &mut self.counter,
                crate::collections::rekey::field_child_id(parent_id, "counter")
            );
        }
    }

    // Structural: a test fixture, merged by the storage layer's own rules.
    #[diagnostic::do_not_recommend]
    impl crate::collections::MergeStrategy for DispatchTestApp {
        const DISPATCHED: bool = false;
    }

    impl Mergeable for DispatchTestApp {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            self.counter.merge(&other.counter)
        }
    }

    /// A request carrying two app-state entries, each a value followed by the entry id.
    fn request(existing: &[u8], incoming: &[u8]) -> MergeRootStateRequest {
        let entry = |value: &[u8]| [value, ROOT_ENTRY_ID.as_bytes()].concat();
        MergeRootStateRequest {
            existing: entry(existing),
            incoming: entry(incoming),
            existing_created_at: 50,
            existing_ts: 100,
            incoming_ts: 200,
        }
    }

    #[test]
    #[serial]
    fn merge_root_state_typed_combines_disjoint_executor_counts() {
        env::reset_for_testing();

        env::set_device_id([1; 32]);
        let mut state_a = DispatchTestApp {
            counter: Counter::new(),
        };
        state_a.counter.increment().unwrap();
        state_a.counter.increment().unwrap();
        let bytes_a = borsh::to_vec(&state_a).unwrap();

        env::set_device_id([2; 32]);
        let mut state_b = DispatchTestApp {
            counter: Counter::new(),
        };
        state_b.counter.increment().unwrap();
        let bytes_b = borsh::to_vec(&state_b).unwrap();

        // A receives B's increments: a G-Counter keeps each executor's own, 2 + 1.
        let MergeRootStateResponse::Ok(merged) =
            merge_root_state_typed::<DispatchTestApp>(&request(&bytes_a, &bytes_b))
        else {
            panic!("the typed merge must succeed");
        };
        let value = merged
            .strip_suffix(ROOT_ENTRY_ID.as_bytes().as_slice())
            .unwrap();
        let merged: DispatchTestApp = borsh::from_slice(value).unwrap();
        assert_eq!(merged.counter.value().unwrap(), 3);
    }

    #[test]
    #[serial]
    fn merge_root_state_typed_takes_the_incoming_entry_over_an_unreadable_stored_one() {
        env::reset_for_testing();
        let valid = borsh::to_vec(&DispatchTestApp {
            counter: Counter::new(),
        })
        .unwrap();
        let request = request(&[0xff; 4], &valid);

        let response = merge_root_state_typed::<DispatchTestApp>(&request);

        assert!(
            matches!(&response, MergeRootStateResponse::Ok(merged) if *merged == request.incoming),
            "a readable incoming entry replaces an unreadable stored one, got {response:?}"
        );
    }

    #[test]
    #[serial]
    fn merge_root_state_typed_refuses_an_incoming_entry_the_app_type_does_not_read() {
        env::reset_for_testing();
        let valid = borsh::to_vec(&DispatchTestApp {
            counter: Counter::new(),
        })
        .unwrap();
        let mut bootstrap = request(&valid, &[0xff; 4]);
        bootstrap.existing_ts = bootstrap.existing_created_at;

        let response = merge_root_state_typed::<DispatchTestApp>(&bootstrap);

        assert!(
            matches!(response, MergeRootStateResponse::Refused(_)),
            "an unreadable incoming entry is refused on a never-written stored one, got {response:?}"
        );
    }

    #[test]
    #[serial]
    fn merge_root_state_refuses_an_incoming_value_the_app_type_does_not_read() {
        env::reset_for_testing();
        clear_merge_registry();
        register_crdt_merge::<DispatchTestApp>();
        let existing = borsh::to_vec(&DispatchTestApp {
            counter: Counter::new(),
        })
        .unwrap();

        // A 0xFFFFFFFF borsh length prefix overruns, so this is no DispatchTestApp.
        let result = merge_root_state(&existing, &[0xff; 4], 50, 100, 200);

        assert!(
            matches!(result, Err(MergeError::SerializationError(_))),
            "an unreadable incoming value must be refused, got {result:?}"
        );
        clear_merge_registry();
    }

    #[test]
    #[serial]
    fn merge_root_state_takes_a_readable_incoming_value_over_an_unreadable_stored_one() {
        env::reset_for_testing();
        clear_merge_registry();
        register_crdt_merge::<DispatchTestApp>();
        let incoming = borsh::to_vec(&DispatchTestApp {
            counter: Counter::new(),
        })
        .unwrap();

        let merged = merge_root_state(&[0xff; 4], &incoming, 50, 200, 100)
            .expect("a readable incoming value replaces an unreadable stored one");

        assert_eq!(merged, incoming);
        clear_merge_registry();
    }

    #[test]
    #[serial]
    fn merge_root_entry_takes_the_incoming_entry_over_a_stored_one_without_its_id() {
        env::reset_for_testing();
        clear_merge_registry();
        register_crdt_merge::<DispatchTestApp>();
        let mut incoming = borsh::to_vec(&DispatchTestApp {
            counter: Counter::new(),
        })
        .unwrap();
        incoming.extend_from_slice(ROOT_ENTRY_ID.as_bytes());

        let merged = merge_root_entry(&[0xff; 4], &incoming, 50, 100, 200)
            .expect("a stored entry without its id loses to a readable incoming one");

        assert_eq!(merged, incoming, "the id goes back on the merged value");
        clear_merge_registry();
    }

    #[test]
    #[serial]
    fn merge_fugue_text_container_leaves_the_stored_document_unchanged() {
        env::reset_for_testing();
        let mut doc = Root::new(|| FugueText::<MainStorage>::new_with_field_name("merge_noop"));
        doc.insert_str_with_replica(0, 1, "hello").unwrap();
        let existing = borsh::to_vec(&*doc).unwrap();

        doc.insert_str_with_replica(5, 2, " world").unwrap();
        let incoming = borsh::to_vec(&*doc).unwrap();
        assert_eq!(
            existing, incoming,
            "a Collection serializes only its element id, so two handles on one \
             document are byte-identical however far their contents have diverged"
        );

        let merged = merge_by_crdt_type(&CrdtType::FugueText, &existing, &incoming)
            .expect("the container arm must succeed");
        assert_eq!(merged, existing, "the container merge must be byte-stable");
        assert_eq!(
            doc.get_text().unwrap(),
            "hello world",
            "the container merge must not touch the stored blocks"
        );
    }
}
