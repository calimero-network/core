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
pub use calimero_primitives::crdt::CrdtType;

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

/// Marker trait for types that can be merged (all CRDTs)
#[diagnostic::on_unimplemented(
    message = "(calimero)> `{Self}` cannot be stored in replicated state — it is not a CRDT",
    label = "this type has no merge semantics",
    note = "every `#[app::state]` field and every collection value must be `Mergeable` so replicas converge.",
    note = "fixes: wrap a plain value in `LwwRegister<{Self}>` (last-write-wins) or `Counter`; \
            use `UnorderedMap`/`UnorderedSet`/`Vector` for collections; or `#[derive(Mergeable)]` \
            on your own struct (every field must itself be `Mergeable`)."
)]
pub trait Mergeable {
    /// Merge with another instance of the same type
    ///
    /// # Errors
    ///
    /// Returns error if merge fails (e.g., incompatible states)
    fn merge(&mut self, other: &Self) -> Result<(), MergeError>;
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
        /// The type name that requires WASM callback
        type_name: String,
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
            MergeError::WasmRequired { type_name } => {
                write!(f, "WASM callback required for type: {type_name}")
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
}
