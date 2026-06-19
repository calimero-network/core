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

/// Implemented by types that carry nested collection ids needing deterministic
/// re-keying relative to their storage parent.
///
/// Built-in collections implement this and self-register in their constructors.
/// Application structs used as CRDT values (e.g. an `UnorderedMap` value that is
/// a `#[derive(Mergeable)]` struct of counters) get a generated impl that
/// re-keys each field under a field-namespaced child id, so every replica
/// derives identical nested ids and the children converge as entities instead of
/// the whole struct blob being last-writer-wins'd. `pub` so generated impls can
/// live in application crates.
///
/// The `Any` supertrait requires `'static` (you cannot impl `Any` — and thus
/// `RekeyTarget` — for a type holding a non-`'static` borrow; that's a compile
/// error at the impl site, E0478). So every `RekeyTarget` type satisfies the
/// `+ 'static` bound the dispatch macros below want — there is no way to write a
/// non-`'static` impl that would silently take the no-op arm.
///
/// `#[doc(hidden)]`: `pub` only so macro-generated `impl`s in app crates can
/// name it; app authors never implement or reference it directly.
#[doc(hidden)]
pub trait RekeyTarget: Any {
    /// Re-key this value's nested collection ids relative to `parent_id` (the
    /// deterministic entity id under which this value is stored). Idempotent.
    fn rekey_relative_to(&mut self, parent_id: Id);

    /// Cascade-register the re-key thunks of the value types nested in THIS
    /// type, so they can be dispatched to when stored as collection values.
    ///
    /// Registration (unlike `rekey_relative_to`) does not recurse on its own:
    /// `rekey_nested_value` dispatches by `TypeId`, so a value type that is
    /// never registered is silently skipped (last-writer-wins'd). The root
    /// `#[app::state]` scan only sees the type tokens written in the root
    /// struct's own fields; a custom struct reachable only *through another
    /// custom struct's collection* (e.g. `Map<_, Outer>` where `Outer` holds
    /// `Map<_, Inner>`) is never named there, so without this hook `Inner`
    /// would keep per-replica-random nested ids and lose concurrent writes.
    ///
    /// `#[derive(Mergeable)]` overrides this to register each of its own
    /// collection-field value types (via `register_rekey_if_supported!`, which
    /// cascades in turn), walking the full value graph one struct at a time.
    /// The default is empty: leaf collections (`Counter`) and types with no
    /// registerable nested value carry nothing further to register.
    ///
    /// This is a type-level operation, so the `Self: Sized` bound is deliberate:
    /// it is invoked ONLY on a concrete type, as `register_rekey_cascade::<T>()`,
    /// never through a `dyn RekeyTarget` trait object (registration has no value
    /// to dispatch on). That bound makes the method object-unsafe, so an attempt
    /// to cascade through a trait object won't compile rather than silently
    /// taking the no-op default. The instance-level `rekey_relative_to` stays
    /// object-safe — it is the one called through the type-erased thunk.
    fn register_nested_value_types()
    where
        Self: Sized,
    {
    }
}

/// Derive a deterministic per-field child id from a parent entity id and a field
/// name, so sibling fields (e.g. two counters) get distinct namespaces and never
/// collide. Public for macro-generated `RekeyTarget` impls.
#[must_use]
pub fn field_child_id(parent_id: Id, field_name: &str) -> Id {
    super::compute_collection_id(Some(parent_id), field_name)
}

/// Register `T`'s thunk and, only on its FIRST registration, cascade into the
/// value types `T` nests (via `T::register_nested_value_types`). This is what
/// `register_rekey_if_supported!` dispatches to, so a single scan of the root
/// state's fields transitively registers the whole reachable custom-struct
/// value graph — not just one level.
///
/// The "first registration only" guard is load-bearing: it makes the walk
/// terminate on self- and mutually-recursive value types (e.g.
/// `Tree { children: UnorderedMap<_, Tree> }`). `register_rekey` reports
/// whether THIS call inserted the thunk; we recurse only then, so an
/// already-registered type is never re-walked.
#[doc(hidden)]
pub fn register_rekey_cascade<T: RekeyTarget + 'static>() {
    if register_rekey::<T>() {
        T::register_nested_value_types();
    }
}

type RekeyThunk = fn(&mut dyn Any, Id);

// Process-global and append-only: there is deliberately NO reset path. A thunk
// is a pure function of its type, so re-registration is idempotent and stale
// entries are impossible — clearing would only add a footgun. Consequence for
// tests: once a type is registered it stays registered for the whole binary, so
// a test that needs the "unregistered" path must use a DISTINCT type (see
// `tests/rekey_record.rs`'s `FixedStats` vs `UnfixedStats`).
static REGISTRY: LazyLock<RwLock<HashMap<TypeId, RekeyThunk>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register `T`'s re-key thunk. Called from `T`'s constructor; idempotent and
/// cheap (a read-lock hit on the common already-registered path). Returns
/// `true` iff THIS call inserted the thunk (it was not already present), so
/// `register_rekey_cascade` can recurse exactly once per type.
pub(crate) fn register_rekey<T: RekeyTarget + 'static>() -> bool {
    let tid = TypeId::of::<T>();
    // Recover from a poisoned lock rather than propagating the panic: the
    // registry is an append-only map of independent fn pointers, so a thread
    // that panicked mid-access left it in a usable state.
    if REGISTRY
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&tid)
    {
        return false;
    }
    // Use the entry API under the write lock (not a second `contains_key`) so
    // the insert-or-not decision is atomic: if two threads both miss the read
    // fast-path for the SAME type, exactly one observes `Vacant` and returns
    // true; the other sees `Occupied` and returns false. So the cascade fires
    // exactly once per type — never twice for one type on a startup race.
    match REGISTRY
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .entry(tid)
    {
        std::collections::hash_map::Entry::Occupied(_) => false,
        std::collections::hash_map::Entry::Vacant(slot) => {
            let _ = slot.insert(|any: &mut dyn Any, parent: Id| {
                if let Some(t) = any.downcast_mut::<T>() {
                    t.rekey_relative_to(parent);
                }
            });
            true
        }
    }
}

/// Re-key any nested collections carried by `value` deterministically relative
/// to `parent_id`. No-op for value types that never registered (leaves, plain
/// data structs). Idempotent.
pub(crate) fn rekey_nested_value<V: 'static>(value: &mut V, parent_id: Id) {
    // Copy the fn pointer out and DROP the read guard before invoking the thunk.
    // This is load-bearing, not incidental: a thunk re-enters the registry — a
    // map/set/vector re-key re-inserts its entries, and `insert` calls
    // `register_rekey`, which takes the WRITE lock. Holding the read guard across
    // the call would deadlock the std `RwLock` on that same-thread upgrade. The
    // statement ends at the `;`, so `thunk` is owned and lock-free below.
    let thunk = REGISTRY
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(&TypeId::of::<V>())
        .copied();
    if let Some(thunk) = thunk {
        thunk(value, parent_id);
    }
}

/// Re-key a struct field's value if its concrete type implements [`RekeyTarget`],
/// else expand to a no-op — without a trait bound at the call site. Resolved via
/// autoref specialization, which requires a CONCRETE type, so this is a macro
/// (a generic fn would resolve the no-op branch once, for all types). Generated
/// `RekeyTarget` impls call it per field.
///
/// `$value` must be a `&mut` place of the field; `$parent` an [`Id`].
///
/// The "real re-key" arm fires for `$value: RekeyTarget + 'static`. That bound
/// is always satisfied by any `RekeyTarget` type: the trait's `Any` supertrait
/// *requires* `'static`, so a non-`'static` `RekeyTarget` impl can't even be
/// written (compile error at the impl site). There is therefore no
/// "non-`'static` type silently takes the no-op arm" hazard here.
///
/// **Each expansion MUST stay in its own block.** The autoref machinery defines
/// local helper traits/structs; the surrounding `{{ … }}` scopes them per
/// invocation, so repeated calls in one `rekey_relative_to` body don't collide.
/// If a refactor ever flattened these into a shared scope, the duplicate names
/// would shadow and could make the no-op arm win silently — which would stop
/// re-keying and regress to the #2577 data loss with NO compile error. Keep the
/// block.
#[macro_export]
macro_rules! rekey_field_if_supported {
    ($value:expr, $parent:expr) => {{
        struct __RkProbe<'a, T: ?::core::marker::Sized>(&'a mut T);
        trait __RkViaRekey {
            fn __rk_go(self, p: $crate::address::Id);
        }
        impl<'a, T: $crate::collections::rekey::RekeyTarget + 'static> __RkViaRekey
            for __RkProbe<'a, T>
        {
            fn __rk_go(self, p: $crate::address::Id) {
                $crate::collections::rekey::RekeyTarget::rekey_relative_to(self.0, p);
            }
        }
        trait __RkViaNoop {
            fn __rk_go(self, p: $crate::address::Id);
        }
        impl<'a, T: ?::core::marker::Sized> __RkViaNoop for &__RkProbe<'a, T> {
            fn __rk_go(self, _p: $crate::address::Id) {}
        }
        __RkProbe($value).__rk_go($parent)
    }};
}

/// Register a value type's re-key thunk if it implements [`RekeyTarget`], else a
/// no-op. Generated registration code calls this for each collection-field value
/// type so app structs auto-register before any insert. Macro (not fn) for the
/// same autoref-on-concrete-type reason as [`rekey_field_if_supported`].
///
/// The real arm goes through `register_rekey_cascade`, which on a type's first
/// registration also registers the value types IT nests
/// ([`RekeyTarget::register_nested_value_types`]). So one call on a root field
/// type transitively covers the whole reachable custom-struct value graph, not
/// just one level.
#[macro_export]
macro_rules! register_rekey_if_supported {
    ($t:ty) => {{
        struct __RgProbe<T>(::core::marker::PhantomData<T>);
        trait __RgViaReg {
            fn __rg_go(self);
        }
        impl<T: $crate::collections::rekey::RekeyTarget + 'static> __RgViaReg for __RgProbe<T> {
            fn __rg_go(self) {
                $crate::collections::rekey::register_rekey_cascade::<T>();
            }
        }
        trait __RgViaNoop {
            fn __rg_go(self);
        }
        impl<T> __RgViaNoop for &__RgProbe<T> {
            fn __rg_go(self) {}
        }
        __RgProbe::<$t>(::core::marker::PhantomData).__rg_go()
    }};
}
