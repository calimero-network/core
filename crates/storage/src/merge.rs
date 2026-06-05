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

// Always-native wrapper for the in-process test harness. Unlike
// `register_crdt_merge` it isn't gated behind the `testing` feature, so an
// app's macro-generated `TestState` bridge compiles under `cargo test`
// regardless of whether that app enabled the feature.
#[cfg(not(target_arch = "wasm32"))]
pub use registry::register_crdt_merge_for_test;

#[cfg(any(test, feature = "testing"))]
pub use registry::clear_merge_registry;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::collections::crdt_meta::{CrdtType, MergeError, Mergeable};
use crate::collections::{Counter, ReplicatedGrowableArray};
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
/// `Ok(bytes)` carries the merged root-state bytes the host writes back
/// into storage. `Err(message)` surfaces a merge failure (typically a
/// deserialization or app-`Mergeable::merge` error) to the host without
/// having to panic in WASM.
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub enum MergeRootStateResponse {
    Ok(Vec<u8>),
    Err(String),
}

/// Typed root-state merge for use inside the WASM module's
/// macro-generated `__calimero_merge_root_state` export.
///
/// Implements the same two-tier dispatch the pre-rewrite host-side
/// `merge_root_state` provided:
///
/// 1. **Bootstrap fast-path** — when `existing_created_at == existing_ts`,
///    the local entity was created but has never been explicitly
///    updated since (the freshly-materialised default state on a
///    joiner). In that case `incoming` carries the only real history
///    and must be accepted unconditionally. Plain CRDT merge would
///    treat the local defaults as a competing branch and produce a
///    union-like result that drops parts of the remote's writes —
///    exactly the regression `kv-store-with-shared-storage` exposes.
///
/// 2. **Typed merge** — deserialize both sides as `T`, run
///    `Mergeable::merge` (wrapped in `with_merge_mode` so timestamp
///    generation is suppressed and the merged hash is deterministic),
///    return serialized bytes.
///
/// # Errors
///
/// Returns `MergeError::SerializationError` if either input bytes
/// fail to deserialize as `T`, or if the merged state fails to
/// re-serialize. Returns whatever error variant
/// `<T as Mergeable>::merge` produces if the app's merge logic fails.
pub fn merge_root_state_typed<T>(
    existing: &[u8],
    incoming: &[u8],
    existing_created_at: u64,
    existing_ts: u64,
    _incoming_ts: u64,
) -> Result<Vec<u8>, MergeError>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
{
    if existing_created_at == existing_ts {
        return Ok(incoming.to_vec());
    }

    let mut existing_state = borsh::from_slice::<T>(existing)
        .map_err(|e| MergeError::SerializationError(format!("existing: {e}")))?;
    let incoming_state = borsh::from_slice::<T>(incoming)
        .map_err(|e| MergeError::SerializationError(format!("incoming: {e}")))?;

    crate::env::with_merge_mode(|| existing_state.merge(&incoming_state))?;

    borsh::to_vec(&existing_state).map_err(|e| MergeError::SerializationError(e.to_string()))
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
/// - The registered merge function fails
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
        registry::MergeRegistryResult::AllFunctionsFailed => {
            // Merge functions are registered, but none could deserialize+merge
            // these bytes. Dispatch is type-blind — there is no per-entity type
            // hint to select a merger by, so every registered fn is tried and
            // this arm is reached whenever none deserializes the bytes. Two
            // cases land here indistinguishably:
            //   * EXPECTED (common): this entity's type simply has no custom
            //     CRDT merge, so plain LWW is the correct resolution — e.g. a
            //     routine `set`, or a root "shell" whose real data lives in
            //     child entities that merge on their own paths.
            //   * RARE: a custom-merge type's bytes are a genuine
            //     mismatch/corruption.
            // Log at DEBUG, not WARN: the benign case dominates (a WARN reading
            // "type mismatch or corrupt data" here made every routine write look
            // like data loss), and a real corruption still surfaces downstream
            // as hash divergence and via the loud `NoMergeFunctionRegistered`
            // error path above (which fires when NO merger is registered).
            tracing::debug!(
                target: "calimero_storage::merge",
                existing_ts,
                incoming_ts,
                "no registered merge function matched this entity; resolving by \
                 LWW (expected when the type has no custom CRDT merge)"
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
        // SortedMap stores and merges exactly like UnorderedMap (entries sync
        // separately; ordering is a read-time concern derived from `K: Ord`), so
        // the container merge is the same add-wins structural pass.
        CrdtType::SortedMap { .. } => merge_unordered_map(existing, incoming),
        CrdtType::UnorderedSet { .. } => merge_unordered_set(existing, incoming),
        // SortedSet stores/merges exactly like UnorderedSet (union; ordering is a
        // read-time concern derived from `T: Ord`).
        CrdtType::SortedSet { .. } => merge_unordered_set(existing, incoming),
        CrdtType::Vector { .. } => merge_vector(existing, incoming),

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

        // RotationLog (P3 of core#2716) - unconditional union of entries by
        // delta_id. Entries are authenticated at resolve time (writers_at), so
        // the merge trusts nothing and just unions.
        CrdtType::RotationLog => merge_rotation_log(existing, incoming),

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

/// Merge two rotation logs (P3 of core#2716).
///
/// Unconditional **union of entries by `delta_id`**, then a canonical sort by
/// `delta_id` so the merged bytes — and therefore the entity hash — are
/// identical regardless of the order in which two nodes apply each other's
/// logs. (Resolution order is HLC-based in `writers_at`, so the *stored* order
/// is semantically irrelevant, but it MUST be canonical for the hash to
/// converge.) Entries are NOT authenticated here — the merge trusts nothing;
/// verification happens at resolve time. The compaction `snapshot` (P6) is
/// carried forward by the larger `cutoff_index`.
fn merge_rotation_log(existing: &[u8], incoming: &[u8]) -> Result<Vec<u8>, MergeError> {
    use crate::rotation_log::RotationLog;

    // Empty bytes = an empty log, NOT a parse error. The first write to a
    // rotation-log child registers the child in the index (so the merge path
    // fires) before the value bytes exist, so the stored side is `&[]` on that
    // first merge; treating it as empty lets the union absorb the incoming log
    // instead of falling back to LWW (which could pick the empty side and wipe
    // the entries).
    let parse = |bytes: &[u8]| -> Result<RotationLog, MergeError> {
        if bytes.is_empty() {
            Ok(RotationLog::empty())
        } else {
            borsh::from_slice(bytes).map_err(|e| MergeError::SerializationError(e.to_string()))
        }
    };

    let mut merged: RotationLog = parse(existing)?;
    let incoming_log: RotationLog = parse(incoming)?;

    let mut seen: std::collections::BTreeSet<[u8; 32]> =
        merged.entries.iter().map(|e| e.delta_id).collect();
    for entry in incoming_log.entries {
        if seen.insert(entry.delta_id) {
            merged.entries.push(entry);
        }
    }
    merged.entries.sort_by(|a, b| a.delta_id.cmp(&b.delta_id));

    if let Some(inc) = incoming_log.snapshot {
        let take = match &merged.snapshot {
            Some(cur) => inc.cutoff_index > cur.cutoff_index,
            None => true,
        };
        if take {
            merged.snapshot = Some(inc);
        }
    }

    borsh::to_vec(&merged).map_err(|e| MergeError::SerializationError(e.to_string()))
}

#[cfg(test)]
mod typed_dispatch_tests {
    use super::*;
    use crate::collections::Counter;
    use crate::env;
    use serial_test::serial;

    // Minimal Mergeable app type for the typed-dispatch test. Counter
    // is the simplest Mergeable that produces an observably-different
    // post-merge state from either input alone.
    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize, Debug)]
    struct DispatchTestApp {
        counter: Counter,
    }

    impl Mergeable for DispatchTestApp {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            self.counter.merge(&other.counter)
        }
    }

    #[test]
    #[serial]
    fn merge_root_state_typed_combines_disjoint_executor_counts() {
        env::reset_for_testing();

        // Executor A: counter incremented twice — value 2.
        env::set_executor_id([1; 32]);
        let mut state_a = DispatchTestApp {
            counter: Counter::new(),
        };
        state_a.counter.increment().unwrap();
        state_a.counter.increment().unwrap();
        let bytes_a = borsh::to_vec(&state_a).unwrap();

        // Executor B: counter incremented once — value 1.
        env::set_executor_id([2; 32]);
        let mut state_b = DispatchTestApp {
            counter: Counter::new(),
        };
        state_b.counter.increment().unwrap();
        let bytes_b = borsh::to_vec(&state_b).unwrap();

        // Typed merge with non-bootstrap timestamps (existing was
        // written, so we want the real CRDT merge, not the fast-path).
        // A receives B's increments. G-Counter union per executor → 2 + 1 = 3.
        let merged_bytes = merge_root_state_typed::<DispatchTestApp>(
            &bytes_a, &bytes_b, /* created_at */ 50, /* existing_ts */ 100,
            /* incoming_ts */ 200,
        )
        .expect("typed merge should succeed");
        let merged: DispatchTestApp = borsh::from_slice(&merged_bytes).unwrap();
        assert_eq!(merged.counter.value().unwrap(), 3);
    }

    #[test]
    #[serial]
    fn merge_root_state_typed_bootstrap_returns_incoming_verbatim() {
        env::reset_for_testing();

        // Bootstrap shape: existing was created but never written
        // (`created_at == existing_ts`). The fast-path must accept
        // incoming bytes verbatim, regardless of whether they'd
        // deserialize as the typed `T` — this is the joiner-bootstrap
        // case the kv-store-with-shared-storage regression exposed.
        let some_bytes = vec![9, 9, 9, 9];
        let incoming = vec![1, 2, 3, 4];

        let out = merge_root_state_typed::<DispatchTestApp>(
            &some_bytes,
            &incoming,
            /* created_at */ 100,
            /* existing_ts */ 100,
            /* incoming_ts */ 50,
        )
        .expect("bootstrap fast-path must succeed");
        assert_eq!(out, incoming, "bootstrap must return incoming verbatim");
    }

    #[test]
    #[serial]
    fn merge_root_state_typed_rejects_malformed_existing() {
        env::reset_for_testing();

        let valid_bytes = borsh::to_vec(&DispatchTestApp {
            counter: Counter::new(),
        })
        .unwrap();
        let bad = vec![0xff, 0xff, 0xff, 0xff];

        // Post-bootstrap timestamps to avoid the fast-path so the
        // typed deserialize is reached.
        let result = merge_root_state_typed::<DispatchTestApp>(
            &bad,
            &valid_bytes,
            /* created_at */ 50,
            /* existing_ts */ 100,
            /* incoming_ts */ 200,
        );
        assert!(
            matches!(result, Err(MergeError::SerializationError(_))),
            "expected SerializationError, got {:?}",
            result
        );
    }

    #[test]
    #[serial]
    fn merge_root_state_typed_rejects_malformed_incoming() {
        env::reset_for_testing();

        let valid_bytes = borsh::to_vec(&DispatchTestApp {
            counter: Counter::new(),
        })
        .unwrap();
        let bad = vec![0xff, 0xff, 0xff, 0xff];

        let result = merge_root_state_typed::<DispatchTestApp>(
            &valid_bytes,
            &bad,
            /* created_at */ 50,
            /* existing_ts */ 100,
            /* incoming_ts */ 200,
        );
        assert!(
            matches!(result, Err(MergeError::SerializationError(_))),
            "expected SerializationError, got {:?}",
            result
        );
    }

    #[test]
    #[serial]
    fn merge_root_state_all_functions_failed_falls_back_to_lww() {
        // The type-blind `merge_root_state` path (vs the typed one above): a
        // merger IS registered, but the bytes don't deserialize as its type, so
        // the registry returns `AllFunctionsFailed` and we resolve by LWW. This
        // pins that fallback after the log-level change — it is the branch a
        // routine `set` exercises in production.
        env::reset_for_testing();
        clear_merge_registry();
        register_crdt_merge::<DispatchTestApp>();

        // Neither side deserializes as DispatchTestApp (a 0xFFFFFFFF borsh
        // length prefix overruns), so the one registered fn fails on both.
        let existing = vec![0xff, 0xff, 0xff, 0xff];
        let incoming = vec![0x01, 0x02, 0x03, 0x04];

        // Non-bootstrap timestamps (created_at != existing_ts) so the
        // accept-incoming fast-path is skipped and we reach the LWW fallback.
        let newer_incoming = merge_root_state(
            &existing, &incoming, /* created_at */ 50, /* existing_ts */ 100,
            /* incoming_ts */ 200,
        )
        .expect("LWW fallback returns Ok");
        assert_eq!(
            newer_incoming, incoming,
            "incoming_ts >= existing_ts → incoming wins"
        );

        let older_incoming = merge_root_state(
            &existing, &incoming, /* created_at */ 50, /* existing_ts */ 200,
            /* incoming_ts */ 100,
        )
        .expect("LWW fallback returns Ok");
        assert_eq!(
            older_incoming, existing,
            "incoming_ts < existing_ts → existing wins"
        );

        clear_merge_registry();
    }

    /// P3 (core#2716): the rotation-log merge must be an order-invariant union
    /// by `delta_id`, so two nodes that apply each other's logs in opposite
    /// orders produce byte-identical results (hence identical hashes → the log
    /// child converges via ordinary sync).
    #[test]
    fn merge_rotation_log_is_order_invariant_union() {
        use core::num::NonZeroU128;
        use std::collections::BTreeMap;

        use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
        use crate::rotation_log::{RotationLog, RotationLogEntry};

        let mk = |delta_id: u8, ns: u64| RotationLogEntry {
            delta_id: [delta_id; 32],
            delta_hlc: HybridTimestamp::new(Timestamp::new(
                NTP64(ns),
                ID::from(NonZeroU128::new(1).unwrap()),
            )),
            signer: None,
            signature: None,
            signed_payload: None,
            new_writers: BTreeMap::new(),
            writers_nonce: 0,
        };

        // Overlapping logs: {01,02} and {02,03} — 02 is shared.
        let a = RotationLog {
            snapshot: None,
            entries: vec![mk(0x01, 10), mk(0x02, 20)],
        };
        let b = RotationLog {
            snapshot: None,
            entries: vec![mk(0x02, 20), mk(0x03, 30)],
        };
        let a_bytes = borsh::to_vec(&a).unwrap();
        let b_bytes = borsh::to_vec(&b).unwrap();

        let ab = merge_rotation_log(&a_bytes, &b_bytes).unwrap();
        let ba = merge_rotation_log(&b_bytes, &a_bytes).unwrap();

        assert_eq!(
            ab, ba,
            "merge must be order-invariant (canonical sort) so the hash converges"
        );

        let merged: RotationLog = borsh::from_slice(&ab).unwrap();
        let ids: Vec<u8> = merged.entries.iter().map(|e| e.delta_id[0]).collect();
        assert_eq!(
            ids,
            vec![0x01, 0x02, 0x03],
            "union deduped by delta_id and sorted canonically"
        );
    }
}
