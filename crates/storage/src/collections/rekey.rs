//! Deterministic re-keying of nested collection ids on insert.
//!
//! # The problem this solves
//!
//! Collections created on-demand at runtime — e.g. `Counter::new()` stored as
//! an `UnorderedMap` value on first touch — get a RANDOM internal collection id
//! (`Collection::new(None)` → `Id::random()`). The storage sync model converges
//! collections by matching entity ids (container merge is add-wins; children
//! sync as separate entities and merge by id). So two nodes that independently
//! first-create the same logical nested CRDT mint DIFFERENT random internal ids,
//! their children land under different parents, and they NEVER merge — a
//! permanent divergence (e.g. a `GCounter` reads 1 instead of 2).
//!
//! `__assign_deterministic_ids` (the `#[app::state]` macro) fixes only
//! TOP-LEVEL state fields. This module extends that to nested values: when a
//! value is inserted into a map/set/vector under a deterministic entity id, its
//! nested collection ids are re-keyed deterministically relative to that id, so
//! every node derives the same ids and the children converge.
//!
//! # Why a TypeId registry instead of a trait bound or `Any`-enumeration
//!
//! A `RekeyNested` trait bound on the insert APIs would be the obvious design,
//! but on stable Rust (no `specialization`) there is no blanket no-op impl, so
//! every value type — including every app-defined one — would have to implement
//! it, breaking existing apps. Enumerating concrete collection types via `Any`
//! downcasts avoids that but is fragile: a new `UnorderedSet<NewType>` value
//! would silently not re-key.
//!
//! Instead, each collection type registers a type-erased re-key thunk keyed by
//! its `TypeId` in its constructor (the only place that mints the random id we
//! need to fix). `rekey_nested_value` looks `V`'s `TypeId` up and invokes the
//! thunk if present. This is:
//! - **source-compatible** — no trait bound on insert (only `V: 'static`),
//!   leaf/app value types simply have no registration and are left untouched;
//! - **not fragile** — any collection instantiation that is ever constructed
//!   registers itself, so coverage is automatic for all `V`;
//! - **sufficient** — re-keying only matters at *first creation* (where the id
//!   would otherwise be random), which always flows through a constructor;
//!   updates of an already-deterministic entity need no re-key.

use core::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use crate::address::Id;

/// Implemented by collection types that carry nested collection ids needing
/// deterministic re-keying relative to their storage parent.
pub(crate) trait RekeyTarget: Any {
    /// Re-key this value's nested collection ids relative to `parent_id` (the
    /// deterministic entity id under which this value is stored). Idempotent.
    fn rekey_relative_to(&mut self, parent_id: Id);
}

type RekeyThunk = fn(&mut dyn Any, Id);

static REGISTRY: LazyLock<RwLock<HashMap<TypeId, RekeyThunk>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register `T`'s re-key thunk. Called from `T`'s constructor; idempotent and
/// cheap (a read-lock hit on the common already-registered path).
pub(crate) fn register_rekey<T: RekeyTarget + 'static>() {
    let tid = TypeId::of::<T>();
    if REGISTRY
        .read()
        .expect("rekey registry poisoned")
        .contains_key(&tid)
    {
        return;
    }
    REGISTRY
        .write()
        .expect("rekey registry poisoned")
        .entry(tid)
        .or_insert(|any: &mut dyn Any, parent: Id| {
            if let Some(t) = any.downcast_mut::<T>() {
                t.rekey_relative_to(parent);
            }
        });
}

/// Re-key any nested collections carried by `value` deterministically relative
/// to `parent_id`. No-op for value types that never registered (leaves, plain
/// data structs). Idempotent.
pub(crate) fn rekey_nested_value<V: 'static>(value: &mut V, parent_id: Id) {
    let thunk = REGISTRY
        .read()
        .expect("rekey registry poisoned")
        .get(&TypeId::of::<V>())
        .copied();
    if let Some(thunk) = thunk {
        thunk(value, parent_id);
    }
}
