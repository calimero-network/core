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
pub trait Mergeable {
    /// Merge with another instance of the same type
    ///
    /// # Errors
    ///
    /// Returns error if merge fails (e.g., incompatible states)
    fn merge(&mut self, other: &Self) -> Result<(), MergeError>;
}

/// CRDT map shape — key/value collection that satisfies the [`Mergeable`] contract.
///
/// Implementors must guarantee that `merge` is associative, commutative, and idempotent
/// over their key/value space. The `Error` associated type lets storage-backed and
/// in-memory implementations coexist.
pub trait CrdtMap: Mergeable {
    type Key;
    type Value;
    type Error;

    fn insert(
        &mut self,
        key: Self::Key,
        value: Self::Value,
    ) -> Result<Option<Self::Value>, Self::Error>;

    fn get(&self, key: &Self::Key) -> Result<Option<Self::Value>, Self::Error>;

    fn remove(&mut self, key: &Self::Key) -> Result<Option<Self::Value>, Self::Error>;

    fn len(&self) -> Result<usize, Self::Error>;
}

/// CRDT sequence shape — indexed collection (Vector, RGA) that satisfies [`Mergeable`].
pub trait CrdtSequence: Mergeable {
    type Element;
    type Error;

    fn push(&mut self, element: Self::Element) -> Result<(), Self::Error>;
    fn get(&self, index: usize) -> Result<Option<Self::Element>, Self::Error>;
    fn update(
        &mut self,
        index: usize,
        element: Self::Element,
    ) -> Result<Option<Self::Element>, Self::Error>;
    fn len(&self) -> Result<usize, Self::Error>;
}

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
            MergeError::StorageError(msg) => write!(f, "Storage error: {}", msg),
            MergeError::TypeMismatch => write!(f, "Cannot merge different CRDT types"),
            MergeError::WasmRequired { type_name } => {
                write!(f, "WASM callback required for type: {}", type_name)
            }
            MergeError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
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
        MergeError::StorageError(format!("{err:?}"))
    }
}

/// Trait for CRDTs that can be decomposed into field entries
///
/// Used for structured storage of nested CRDTs.
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
    fn decompose(&self) -> Result<Vec<(Self::Key, Self::Value)>, DecomposeError>;

    /// Reconstruct from field entries
    ///
    /// # Errors
    ///
    /// Returns error if reconstruction fails
    fn recompose(entries: Vec<(Self::Key, Self::Value)>) -> Result<Self, DecomposeError>
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
            DecomposeError::MissingField(field) => write!(f, "Missing field: {}", field),
            DecomposeError::InvalidValue(msg) => write!(f, "Invalid value: {}", msg),
            DecomposeError::StorageError(msg) => write!(f, "Storage error: {}", msg),
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
    fn crdt_map_trait_shape_compiles() {
        fn _assert_subtrait<T: CrdtMap>() {
            fn _is_mergeable<U: Mergeable>() {}
            _is_mergeable::<T>();
        }
    }

    #[test]
    fn crdt_sequence_trait_shape_compiles() {
        fn _assert_subtrait<T: CrdtSequence>() {
            fn _is_mergeable<U: Mergeable>() {}
            _is_mergeable::<T>();
        }
    }

    #[test]
    fn store_error_converts_into_merge_storage_error() {
        let store_err = crate::collections::error::StoreError::ArithmeticOverflow(
            "overflow while computing collection size".to_owned(),
        );
        let original = format!("{store_err:?}");

        let merge_err: MergeError = store_err.into();

        match merge_err {
            MergeError::StorageError(msg) => {
                assert_eq!(msg, original, "expected debug formatting to be preserved");
            }
            other => panic!("expected MergeError::StorageError, got {other:?}"),
        }
    }
}
