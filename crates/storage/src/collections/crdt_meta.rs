//! CRDT Type System - Metadata and traits for nested CRDT support
//!
//! This module provides the foundation for detecting and handling nested CRDTs,
//! enabling proper field-level merging and storage without blob serialization.
//!
//! # Architecture
//!
//! All CRDT types implement `CrdtMeta` which provides:
//! - Type identification (Counter, Map, Vector, etc.)
//! - Merge semantics (field-level vs whole-value)
//! - Serialization strategy (structured vs blob)

use borsh::{BorshDeserialize, BorshSerialize};

// Re-export the unified CrdtType from primitives
pub use calimero_primitives::crdt::{CrdtType, CustomTypeId};

/// Storage strategy for a CRDT type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageStrategy {
    /// Store as opaque blob (simple types, backward compat)
    Blob,
    /// Store fields separately with composite keys
    Structured,
}

/// Metadata about a CRDT type - implemented by all CRDTs
///
/// This trait enables:
/// - Runtime CRDT type detection
/// - Automatic nested storage handling
/// - Type-aware merge strategies
pub trait CrdtMeta {
    /// Returns the CRDT type identifier
    fn crdt_type() -> CrdtType
    where
        Self: Sized;

    /// Returns the storage strategy for this CRDT
    ///
    /// Structured types (Map, Vector) store fields separately.
    /// Blob types (Counter, LwwRegister) serialize as single values.
    fn storage_strategy() -> StorageStrategy
    where
        Self: Sized,
    {
        StorageStrategy::Blob
    }

    /// Check if this type is a CRDT (always true for implementors)
    fn is_crdt() -> bool
    where
        Self: Sized,
    {
        true
    }

    /// Returns true if this CRDT can contain nested CRDTs
    ///
    /// Collections (Map, Vector, Set) can contain CRDTs.
    /// Registers and Counters cannot.
    fn can_contain_crdts() -> bool
    where
        Self: Sized,
    {
        false
    }
}

/// # Declaring a strategy is required
///
/// A `Mergeable` impl alone does not say whether anything will CALL it. Stored
/// as a collection value, a type that declares nothing resolves last-write-wins
/// and its `merge` never runs — it compiles, type-checks, and decides nothing.
/// That is not hypothetical: this crate's own reference app shipped in exactly
/// that state, and so did the `Entry`-API insert path inside this crate.
///
/// So [`MergeStrategy`] is a supertrait, and only the two macros supply it:
///
/// - `#[derive(Mergeable)]` — structural. Field-by-field delegation, which the
///   storage layer reaches on its own, so nothing is dispatched.
/// - `#[app::mergeable]` — dispatched. The type gets a `CustomTypeId`, entries
///   holding it are stamped, and the merge point calls this rule.
///
/// Hand-writing `impl Mergeable` without either is a compile error. Root state
/// is unaffected in behaviour — it has its own merge path — but still declares,
/// so the distinction is recorded on every type rather than assumed.
///
/// Marker trait for types that can be merged (all CRDTs).
///
/// `RekeyTarget` is a **supertrait**: a `Mergeable` type that nests a collection
/// must re-key it deterministically or it diverges permanently with no runtime
/// error. The bound forces a hand-written `impl Mergeable` to also `impl
/// RekeyTarget` — but only checks the impl EXISTS, not that its body re-keys
/// every field or that the type is registered (a runtime lookup; see `rekey`).
/// `#[derive(Mergeable)]` / `#[app::state]` generate both; leaves no-op.
#[diagnostic::on_unimplemented(
    message = "(calimero)> `{Self}` cannot be stored in replicated state — it is not a CRDT",
    label = "this type has no merge semantics",
    note = "every `#[app::state]` field and every collection value must be `Mergeable` so replicas converge.",
    note = "fixes: wrap a plain value in `LwwRegister<{Self}>` (last-write-wins) or `Counter`; \
            use `UnorderedMap`/`UnorderedSet`/`Vector` for collections; or `#[derive(Mergeable)]` \
            on your own struct (every field must itself be `Mergeable`)."
)]
pub trait Mergeable: crate::collections::rekey::RekeyTarget + MergeStrategy {
    /// Merge with another instance of the same type
    ///
    /// # Errors
    ///
    /// Returns error if merge fails (e.g., incompatible states)
    fn merge(&mut self, other: &Self) -> Result<(), MergeError>;
}

/// How a type's [`Mergeable::merge`] is reached when it is stored as a
/// collection VALUE.
///
/// `Mergeable` alone does not answer that, and the difference is not cosmetic.
/// A collection entry is merged by matching on its `crdt_type`; a value type
/// that declares nothing resolves last-write-wins with its `merge` never
/// consulted. So a hand-written rule can compile, type-check, pass a
/// convergence test, and still not be the thing deciding the outcome — which is
/// exactly what shipped here for months.
///
/// Implemented ONLY by the two macros, which is the point: a hand-written
/// `impl Mergeable` gets neither, and the compiler asks which you meant rather
/// than letting the question go unasked.
///
/// - `#[derive(Mergeable)]` → structural. Field-by-field delegation, the same
///   answer the storage layer reaches on its own, so nothing is dispatched and
///   nothing is lost.
/// - `#[app::mergeable]` → dispatched. The type carries a [`CustomTypeId`], the
///   entry is stamped with it, and the merge point calls this rule.
///
/// A type used ONLY as root state needs neither — the root has its own merge
/// path — which is why this is required at the collection-value position rather
/// than as a supertrait of `Mergeable`.
#[diagnostic::on_unimplemented(
    message = "(calimero)> `{Self}` implements `Mergeable` but never declared how it merges",
    label = "needs `#[app::mergeable]` or `#[derive(Mergeable)]`",
    note = "Stored as a COLLECTION VALUE, a type that declares nothing resolves \
            last-write-wins and its `merge` is never called — it compiles and silently \
            decides nothing. Root state has its own merge path and is unaffected, but the \
            declaration is still required so the distinction is recorded rather than assumed.",
    note = "Add `#[app::mergeable]` to have the merge dispatched, or `#[derive(Mergeable)]` \
            if plain field-by-field delegation is what you want (the storage layer reaches \
            that answer anyway, with no wasm call)."
)]
pub trait MergeStrategy {
    /// Whether the merge point calls this type's own rule.
    ///
    /// `false` means the type converges structurally and its `merge` runs only
    /// on a root-blob conflict.
    const DISPATCHED: bool;
}

/// An app-defined type whose [`Mergeable::merge`] is dispatched at merge time.
///
/// [`Mergeable`] alone is not enough. A collection entry is merged by matching
/// on its `crdt_type`, so a type the storage layer cannot recognise resolves
/// last-write-wins and the app's `merge` is never consulted. Implementing this
/// gives the type a [`CustomTypeId`] to be stamped and dispatched on.
///
/// Emitted by `#[app::mergeable]`. Implementing it by hand means owning the id,
/// which is wire format — see [`CustomTypeId`].
///
/// # Contract
///
/// Dispatch hands merge authority to app code, so `merge` must be
/// **deterministic**, **commutative**, **associative**, **idempotent** and
/// **total**. The last one is the trap: `Err` is not validation, it is a
/// refusal to converge — the entity stays divergent and repair retries it
/// indefinitely. Reject bad input on the write path, not here.
pub trait CustomMergeable: 'static {
    /// Stable identity for this type on the wire.
    const TYPE_ID: CustomTypeId;

    /// Register this type's merge under [`Self::TYPE_ID`].
    ///
    /// Returns whether this was a NEW registration, so the cascade walk
    /// terminates on a self-referential value graph.
    ///
    /// The body is macro-generated, which is the point: it needs `Mergeable`
    /// and both borsh bounds, and carrying those as SUPERTRAITS would make
    /// merely asking "does `T` implement this?" evaluate them. The registration
    /// walk asks that of every field type reachable from the app state,
    /// including types whose borsh impl is itself broken — and each such
    /// question would then re-report that break. It cost a duplicated
    /// `Authorizer` diagnostic before the bounds moved here.
    fn register_merge() -> bool;
}

// Feature-insensitive compile guard for the `Mergeable: RekeyTarget` supertrait.
// This body type-checks only while the bound holds; removing it breaks the build
// in every feature set. Complements the `testing`-gated trybuild negative case
// (which only runs when `testing` is on). Never called.
#[allow(dead_code)]
fn _mergeable_requires_rekeytarget<T: Mergeable>() {
    fn assert_rekey<U: crate::collections::rekey::RekeyTarget>() {}
    assert_rekey::<T>();
}

/// Marker for types usable as a **key** in a Calimero collection
/// (`UnorderedMap`/`SortedMap` keys, `UnorderedSet`/`SortedSet` elements).
///
/// Keys are addressed by their byte representation, so the type must be
/// `AsRef<[u8]>` (plus borsh-(de)serializable, `PartialEq`, and `'static` — the
/// requirements every key path already imposes). This is an SDK-owned alias over
/// those bounds whose only job is to carry a clear diagnostic: a numeric key
/// like `u64` satisfies everything *except* `AsRef<[u8]>` and would otherwise
/// fail with a bare "`AsRef<[u8]>` is not implemented" error at some method call.
/// Blanket-implemented, so it is exactly as permissive as the bounds it names.
#[diagnostic::on_unimplemented(
    message = "(calimero)> `{Self}` can't be used as a collection key — keys must be byte-encodable",
    label = "not a storage key",
    note = "collection keys are addressed by their bytes, so the key type must implement \
            `AsRef<[u8]>` (and be borsh-(de)serializable + `PartialEq` + `'static`). Use `String`, \
            `Vec<u8>`, a `[u8; N]`, or a newtype that implements `AsRef<[u8]>`; a numeric key needs \
            an explicit byte encoding."
)]
pub trait StorageKey:
    BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static
{
}

impl<T: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static> StorageKey for T {}

/// Errors that can occur during CRDT merging
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    /// Incompatible CRDT states (shouldn't happen in practice)
    IncompatibleStates,
    /// Storage error during merge
    StorageError(String),
    /// Type mismatch (attempted to merge different CRDT types)
    TypeMismatch,
    /// WASM callback required for this type.
    ///
    /// The storage layer cannot merge this type without knowing the concrete type.
    /// Examples: `Custom` types, collections with nested generics, `UserStorage<T>`.
    WasmRequired {
        /// The app-defined type that requires a WASM callback.
        ///
        /// A digest rather than a name: the id is what the entry carries, and
        /// resolving it back to a spelling is the guest's job.
        type_id: CustomTypeId,
    },
    /// Serialization/deserialization error during merge.
    SerializationError(String),
    /// No merge function registered for root entity.
    ///
    /// This error enforces I5 (No Silent Data Loss) by failing loudly
    /// when a root entity merge is attempted without a registered merge function.
    ///
    /// **Fix:** Use `#[app::state]` macro or call `register_crdt_merge::<YourState>()`.
    NoMergeFunctionRegistered,
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::IncompatibleStates => write!(f, "Incompatible CRDT states"),
            MergeError::StorageError(msg) => write!(f, "Storage error: {msg}"),
            MergeError::TypeMismatch => write!(f, "Cannot merge different CRDT types"),
            MergeError::WasmRequired { type_id } => {
                write!(
                    f,
                    "WASM callback required for type id {:#018x}",
                    type_id.get()
                )
            }
            MergeError::SerializationError(msg) => write!(f, "Serialization error: {msg}"),
            MergeError::NoMergeFunctionRegistered => {
                write!(
                    f,
                    "No merge function registered for root entity. \
                     Use #[app::state] macro or call register_crdt_merge::<YourState>()."
                )
            }
        }
    }
}

impl std::error::Error for MergeError {}

impl From<crate::collections::error::StoreError> for MergeError {
    fn from(err: crate::collections::error::StoreError) -> Self {
        MergeError::StorageError(format!("{err}"))
    }
}

/// Trait for CRDTs that can be decomposed into field entries
///
/// Used for structured storage of nested CRDTs.
/// A flat list of decomposed `(key, value)` field entries.
pub type DecomposedEntries<K, V> = Vec<(K, V)>;

pub trait Decomposable {
    /// The key type for decomposed entries
    type Key: AsRef<[u8]> + BorshSerialize + BorshDeserialize;
    /// The value type for decomposed entries
    type Value: BorshSerialize + BorshDeserialize;

    /// Decompose into field entries for storage
    ///
    /// # Errors
    ///
    /// Returns error if decomposition fails
    fn decompose(&self) -> Result<DecomposedEntries<Self::Key, Self::Value>, DecomposeError>;

    /// Reconstruct from field entries
    ///
    /// # Errors
    ///
    /// Returns error if reconstruction fails
    fn recompose(
        entries: DecomposedEntries<Self::Key, Self::Value>,
    ) -> Result<Self, DecomposeError>
    where
        Self: Sized;
}

/// Errors during decomposition/recomposition
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecomposeError {
    /// Missing required field
    MissingField(String),
    /// Invalid field value
    InvalidValue(String),
    /// Storage operation failed
    StorageError(String),
}

impl std::fmt::Display for DecomposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecomposeError::MissingField(field) => write!(f, "Missing field: {field}"),
            DecomposeError::InvalidValue(msg) => write!(f, "Invalid value: {msg}"),
            DecomposeError::StorageError(msg) => write!(f, "Storage error: {msg}"),
        }
    }
}

impl std::error::Error for DecomposeError {}

// ============================================================================
// Default implementations for primitive types (non-CRDTs)
// ============================================================================

/// Marker trait for non-CRDT types
pub trait NonCrdt {}

// Implement for common types
impl NonCrdt for String {}
impl NonCrdt for u8 {}
impl NonCrdt for u16 {}
impl NonCrdt for u32 {}
impl NonCrdt for u64 {}
impl NonCrdt for u128 {}
impl NonCrdt for i8 {}
impl NonCrdt for i16 {}
impl NonCrdt for i32 {}
impl NonCrdt for i64 {}
impl NonCrdt for i128 {}
impl NonCrdt for bool {}
impl NonCrdt for char {}

impl<T: NonCrdt> NonCrdt for Vec<T> {}
impl<T: NonCrdt> NonCrdt for Option<T> {}
impl<K: NonCrdt, V: NonCrdt> NonCrdt for std::collections::HashMap<K, V> {}
impl<K: NonCrdt, V: NonCrdt> NonCrdt for std::collections::BTreeMap<K, V> {}

/// Helper macro to check if a type is a CRDT at compile time.
///
/// Returns `true` if the type implements the `CrdtMeta` trait and is marked as a CRDT.
#[macro_export]
macro_rules! is_crdt {
    ($t:ty) => {
        <$t as $crate::collections::crdt_meta::CrdtMeta>::is_crdt()
    };
}

/// Whole-record last-write-wins `Mergeable` for a leaf struct stored as a
/// collection value but NOT made of CRDT fields (e.g. an immutable upload record
/// keyed by a monotonic `uploaded_at`). Emits the LWW `Mergeable` and a matching
/// no-op `RekeyTarget` in one line.
///
/// MUST only be used on a struct with NO collection fields: the emitted
/// `RekeyTarget` is an unconditional no-op, so a nested collection would silently
/// never re-key (the #2577 divergence) and the macro can't check this.
///
/// `$t: Clone`, `$tie` monotonic; `other` replaces `self` iff `other.$tie > self.$tie`.
///
/// ```ignore
/// calimero_storage::impl_atomic_lww_leaf!(FileRecord, uploaded_at);
/// ```
#[macro_export]
macro_rules! impl_atomic_lww_leaf {
    ($t:ty, $tie:ident) => {
        // Structural: this is a leaf that resolves by its own tie-breaker, not
        // an app rule the merge point dispatches to. Declared here so the macro
        // satisfies `Mergeable`'s requirement on its users' behalf.
        impl $crate::collections::MergeStrategy for $t {
            const DISPATCHED: bool = false;
        }

        impl $crate::collections::Mergeable for $t {
            fn merge(
                &mut self,
                other: &Self,
            ) -> ::core::result::Result<(), $crate::collections::crdt_meta::MergeError> {
                // Last-write-wins by the monotonic tie-breaker. Strict `>` keeps
                // `self` on ties, so the merge is idempotent and the outcome is
                // independent of which side is `self` for distinct tie values.
                if other.$tie > self.$tie {
                    *self = ::core::clone::Clone::clone(other);
                }
                ::core::result::Result::Ok(())
            }
        }

        // Whole-record-LWW leaf: no nested collection id to re-key, so the
        // no-op `rekey_relative_to` is correct. Emitted here (not by the app)
        // so the author writes no `RekeyTarget` code.
        impl $crate::collections::rekey::RekeyTarget for $t {
            fn rekey_relative_to(&mut self, _parent_id: $crate::address::Id) {}
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_error_converts_into_merge_storage_error() {
        let store_err = crate::collections::error::StoreError::ArithmeticOverflow(
            "overflow while computing collection size".to_owned(),
        );
        let display_form = format!("{store_err}");

        let merge_err: MergeError = store_err.into();

        match merge_err {
            MergeError::StorageError(msg) => {
                assert_eq!(
                    msg, display_form,
                    "From<StoreError> must use Display so the thiserror message chain is preserved"
                );
                assert!(
                    msg.contains("overflow while computing collection size"),
                    "original error payload must survive the conversion, got: {msg}"
                );
            }
            other => panic!("expected MergeError::StorageError, got {other:?}"),
        }
    }

    // A leaf record merged by `impl_atomic_lww_leaf!`, mirroring an app upload record:
    // plain non-CRDT fields, replaced atomically by a monotonic tie-breaker.
    #[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
    struct Upload {
        name: String,
        size: u64,
        uploaded_at: u64,
    }
    crate::impl_atomic_lww_leaf!(Upload, uploaded_at);

    #[test]
    fn impl_atomic_lww_leaf_is_last_write_wins_by_tie_field() {
        use crate::collections::Mergeable;

        let older = Upload {
            name: "a".to_owned(),
            size: 1,
            uploaded_at: 10,
        };
        let newer = Upload {
            name: "b".to_owned(),
            size: 2,
            uploaded_at: 20,
        };

        // Newer (higher tie) wins regardless of merge direction.
        let mut x = older.clone();
        x.merge(&newer).unwrap();
        assert_eq!(x, newer, "higher uploaded_at must replace self");

        let mut y = newer.clone();
        y.merge(&older).unwrap();
        assert_eq!(y, newer, "lower uploaded_at must NOT replace a newer self");

        // Idempotent / order-independent for distinct tie values.
        let mut z = older.clone();
        z.merge(&newer).unwrap();
        z.merge(&newer).unwrap();
        assert_eq!(z, newer, "repeated merge stays at the winner");
    }

    #[test]
    fn impl_atomic_lww_leaf_emits_a_noop_rekey_target() {
        // The macro emits `RekeyTarget` (supertrait of `Mergeable`); a leaf has
        // no nested id to re-key, so `rekey_relative_to` is a no-op that leaves
        // the value byte-identical.
        use crate::collections::rekey::RekeyTarget;

        let mut u = Upload {
            name: "x".to_owned(),
            size: 7,
            uploaded_at: 3,
        };
        let before = u.clone();
        u.rekey_relative_to(crate::address::Id::root());
        assert_eq!(u, before, "atomic-LWW leaf re-key must be a no-op");
    }
}
