//! Dispatch table for app-defined merge, keyed by [`CustomTypeId`].
//!
//! Sibling of [`super::registry`], which keys the ROOT state's merge by
//! `TypeId`. A root has exactly one type per app, so the runtime's `TypeId` is
//! identity enough. A collection value does not: the entry carries its type as
//! a digest stamped in metadata, and the digest is what arrives here — a
//! `TypeId` is process-local and never crosses a boundary.
//!
//! Both tables are populated inside WASM at module load, from the
//! `__calimero_register_merge` export, and are read by `Interface::save_internal`
//! running in that same instance during delta apply.

#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
use core::any::TypeId;
#[cfg(test)]
use core::cell::RefCell;
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
use std::collections::HashMap;
#[cfg(all(any(target_arch = "wasm32", feature = "testing"), not(test)))]
use std::sync::{LazyLock, RwLock};

use crate::collections::crdt_meta::{CustomMergeable, CustomTypeId, MergeError};

/// Merges two serialized values of one app-defined type.
///
/// Timestamps are deliberately absent: dispatch means the app's rule decides,
/// and a rule that consults wall-clock ordering is not commutative.
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
type CustomMergeFn = fn(&[u8], &[u8]) -> Result<Vec<u8>, MergeError>;

/// Production registry — process-global, shared across async workers.
#[cfg(all(any(target_arch = "wasm32", feature = "testing"), not(test)))]
static CUSTOM_MERGE_REGISTRY: LazyLock<RwLock<HashMap<CustomTypeId, CustomMergeFn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

// Test registry — per-thread, for the same reason `MERGE_REGISTRY` is: `cargo
// test` runs tests in parallel, and a global table lets one test's clear wipe
// another's registration mid-flight.
#[cfg(test)]
thread_local! {
    static CUSTOM_MERGE_REGISTRY: RefCell<HashMap<CustomTypeId, CustomMergeFn>> =
        RefCell::new(HashMap::new());
}

#[cfg(all(any(target_arch = "wasm32", feature = "testing"), not(test)))]
fn with_registry<R>(f: impl FnOnce(&HashMap<CustomTypeId, CustomMergeFn>) -> R) -> R {
    let registry = CUSTOM_MERGE_REGISTRY.read().unwrap_or_else(|_| {
        tracing::error!(
            target: "calimero_storage::merge",
            "CUSTOM_MERGE_REGISTRY lock poisoned during read, aborting."
        );
        std::process::abort()
    });
    f(&registry)
}

#[cfg(all(any(target_arch = "wasm32", feature = "testing"), not(test)))]
fn with_registry_mut<R>(f: impl FnOnce(&mut HashMap<CustomTypeId, CustomMergeFn>) -> R) -> R {
    let mut registry = CUSTOM_MERGE_REGISTRY.write().unwrap_or_else(|_| {
        tracing::error!(
            target: "calimero_storage::merge",
            "CUSTOM_MERGE_REGISTRY lock poisoned during write, aborting."
        );
        std::process::abort()
    });
    f(&mut registry)
}

#[cfg(test)]
fn with_registry<R>(f: impl FnOnce(&HashMap<CustomTypeId, CustomMergeFn>) -> R) -> R {
    CUSTOM_MERGE_REGISTRY.with(|r| f(&r.borrow()))
}

#[cfg(test)]
fn with_registry_mut<R>(f: impl FnOnce(&mut HashMap<CustomTypeId, CustomMergeFn>) -> R) -> R {
    CUSTOM_MERGE_REGISTRY.with(|r| {
        let mut borrowed = r
            .try_borrow_mut()
            .unwrap_or_else(|e| panic!("CUSTOM_MERGE_REGISTRY re-entered during dispatch: {e}"));
        f(&mut borrowed)
    })
}

/// Rust type -> wire id, for stamping an entry at insert time.
///
/// Separate from the dispatch table because the two are looked up by different
/// keys and at different moments. Dispatch arrives from the wire holding a
/// [`CustomTypeId`]; stamping happens inside a generic `insert<V>` that knows
/// only `V`, and cannot name a trait bound to ask `V` directly — the same
/// constraint `rekey_nested_value` solves the same way.
#[cfg(all(any(target_arch = "wasm32", feature = "testing"), not(test)))]
static CUSTOM_TYPE_IDS: LazyLock<RwLock<HashMap<TypeId, CustomTypeId>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

#[cfg(test)]
thread_local! {
    static CUSTOM_TYPE_IDS: RefCell<HashMap<TypeId, CustomTypeId>> =
        RefCell::new(HashMap::new());
}

/// The wire id `V` was registered under, if it declared one.
///
/// Callable from a generic `insert<V>`: it keys on `TypeId`, so it needs no
/// bound on `V` beyond `'static`.
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
#[must_use]
pub fn custom_type_id_of<V: 'static>() -> Option<CustomTypeId> {
    #[cfg(not(test))]
    {
        CUSTOM_TYPE_IDS
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&TypeId::of::<V>())
            .copied()
    }
    #[cfg(test)]
    {
        CUSTOM_TYPE_IDS.with(|m| m.borrow().get(&TypeId::of::<V>()).copied())
    }
}

/// Host build: nothing registers, so nothing is stamped.
#[cfg(not(any(target_arch = "wasm32", test, feature = "testing")))]
#[must_use]
pub const fn custom_type_id_of<V: 'static>() -> Option<CustomTypeId> {
    None
}

/// Register `T`'s merge under its [`CustomMergeable::TYPE_ID`].
///
/// Returns whether this was a NEW registration. The cascade walk offers every
/// reachable type repeatedly, so the flag is what stops a self-referential
/// value graph (`Tree { children: UnorderedMap<_, Tree> }`) from recursing
/// forever — the same guard `register_rekey_cascade` uses.
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
pub fn register_custom_merge<T>() -> bool
where
    T: CustomMergeable
        + crate::collections::Mergeable
        + borsh::BorshSerialize
        + borsh::BorshDeserialize,
{
    let merge_fn: CustomMergeFn = |existing, incoming| {
        // These are ENTRY bytes, not value bytes: `borsh(item) ++ element id`,
        // and for a map `item` is `(V, K)`. Only the leading `V` is ours — the
        // key and the id belong to the collection and must come back byte for
        // byte.
        //
        // Hence reader-decoding rather than `from_slice`, which rejects
        // trailing bytes. The reader stops exactly where `V` ends, which is the
        // whole reason entries are stored value-first: at offset 0 a value is
        // decodable without knowing the key's type.
        let mut existing_rest = existing;
        let mut existing_value = T::deserialize_reader(&mut existing_rest)
            .map_err(|e| MergeError::SerializationError(format!("existing: {e}")))?;

        let incoming_value = T::deserialize_reader(&mut &incoming[..])
            .map_err(|e| MergeError::SerializationError(format!("incoming: {e}")))?;

        // Merge mode suppresses timestamp generation. Without it each replica
        // stamps its own wall clock during the merge and the resulting bytes
        // differ, so identical logical state hashes differently.
        crate::env::with_merge_mode(|| existing_value.merge(&incoming_value))?;

        // The merged value, then `existing`'s untouched tail. Taking the tail
        // from `existing` rather than `incoming` keeps the entry's own id: both
        // sides describe the same entity, so the key agrees, but the id is this
        // replica's and is not the merge's to change.
        let mut out = borsh::to_vec(&existing_value)
            .map_err(|e| MergeError::SerializationError(e.to_string()))?;
        out.extend_from_slice(existing_rest);
        Ok(out)
    };

    // Both tables or neither: a stamped entry whose id has no merge function
    // would dispatch to nothing, and a registered merge whose type is never
    // stamped would never be reached.
    #[cfg(not(test))]
    {
        let _ = CUSTOM_TYPE_IDS
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(TypeId::of::<T>(), T::TYPE_ID);
    }
    #[cfg(test)]
    CUSTOM_TYPE_IDS.with(|m| {
        let _ = m.borrow_mut().insert(TypeId::of::<T>(), T::TYPE_ID);
    });

    with_registry_mut(|registry| registry.insert(T::TYPE_ID, merge_fn).is_none())
}

/// Dispatch a merge for the type `type_id` names.
///
/// `Err(WasmRequired)` means nothing claimed the id — the entry was stamped by
/// a build whose app registered this type and is being merged by one that does
/// not, which is app-upgrade skew rather than a merge failure.
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
pub fn merge_custom(
    type_id: CustomTypeId,
    existing: &[u8],
    incoming: &[u8],
) -> Result<Vec<u8>, MergeError> {
    let merge_fn = with_registry(|registry| registry.get(&type_id).copied());

    merge_fn.map_or(Err(MergeError::WasmRequired { type_id }), |merge_fn| {
        merge_fn(existing, incoming)
    })
}

/// Host build: there is no registry, because a production host has no WASM
/// instance in scope at `save_internal`. Reaching app code from here needs a
/// callback into the guest, which is a separate path — so this reports the id
/// it could not dispatch rather than pretending to resolve it.
#[cfg(not(any(target_arch = "wasm32", test, feature = "testing")))]
pub const fn merge_custom(
    type_id: CustomTypeId,
    _existing: &[u8],
    _incoming: &[u8],
) -> Result<Vec<u8>, MergeError> {
    Err(MergeError::WasmRequired { type_id })
}

/// Whether any app-defined merge is registered in this instance.
#[cfg(any(target_arch = "wasm32", test, feature = "testing"))]
#[must_use]
pub fn has_custom_merges() -> bool {
    with_registry(|registry| !registry.is_empty())
}

/// Host build: no registry, so never.
#[cfg(not(any(target_arch = "wasm32", test, feature = "testing")))]
#[must_use]
pub const fn has_custom_merges() -> bool {
    false
}

/// Drop every registration. Tests only — see the thread-local note above.
#[cfg(any(test, feature = "testing"))]
pub fn clear_custom_merge_registry() {
    with_registry_mut(HashMap::clear);
}

/// No-op counterpart for host builds without the registry.
///
/// A production host never dispatches app merge — it has no WASM instance in
/// scope at `save_internal` — so the symbol exists to keep the registration
/// walk callable from any build rather than to do anything.
#[cfg(not(any(target_arch = "wasm32", test, feature = "testing")))]
#[allow(
    clippy::missing_const_for_fn,
    reason = "signature parity with the real one"
)]
pub fn register_custom_merge<T>() -> bool
where
    T: CustomMergeable
        + crate::collections::Mergeable
        + borsh::BorshSerialize
        + borsh::BorshDeserialize,
{
    false
}
