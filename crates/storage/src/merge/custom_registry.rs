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
        let mut existing_value = borsh::from_slice::<T>(existing)
            .map_err(|e| MergeError::SerializationError(format!("existing: {e}")))?;
        let incoming_value = borsh::from_slice::<T>(incoming)
            .map_err(|e| MergeError::SerializationError(format!("incoming: {e}")))?;

        // Merge mode suppresses timestamp generation. Without it each replica
        // stamps its own wall clock during the merge and the resulting bytes
        // differ, so identical logical state hashes differently.
        crate::env::with_merge_mode(|| existing_value.merge(&incoming_value))?;

        borsh::to_vec(&existing_value).map_err(|e| MergeError::SerializationError(e.to_string()))
    };

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
