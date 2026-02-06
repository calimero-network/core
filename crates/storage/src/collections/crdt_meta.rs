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

/// Describes the Borsh serialization format of an inner type for generic CRDTs.
///
/// This enables `merge_by_crdt_type` to correctly deserialize generic types like
/// `LwwRegister<T>` without knowing `T` at compile time.
///
/// # Examples
///
/// ```ignore
/// // For LwwRegister<u64>:
/// CrdtType::LwwRegister { inner: InnerType::U64 }
///
/// // For LwwRegister<String>:
/// CrdtType::LwwRegister { inner: InnerType::String }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub enum InnerType {
    // Unsigned integers
    /// `u8` - 1 byte fixed size
    U8,
    /// `u16` - 2 bytes fixed size
    U16,
    /// `u32` - 4 bytes fixed size
    U32,
    /// `u64` - 8 bytes fixed size
    U64,
    /// `u128` - 16 bytes fixed size
    U128,

    // Signed integers
    /// `i8` - 1 byte fixed size
    I8,
    /// `i16` - 2 bytes fixed size
    I16,
    /// `i32` - 4 bytes fixed size
    I32,
    /// `i64` - 8 bytes fixed size
    I64,
    /// `i128` - 16 bytes fixed size
    I128,

    // Floats
    /// `f32` - 4 bytes fixed size
    F32,
    /// `f64` - 8 bytes fixed size
    F64,

    // Other primitives
    /// `bool` - 1 byte
    Bool,
    /// `String` - length-prefixed UTF-8 bytes
    String,
    /// `Vec<u8>` - length-prefixed raw bytes
    Bytes,

    /// Custom/unknown type - cannot merge at storage level, requires WASM callback
    Custom(std::string::String),
}

/// Trait for types that can be described as an `InnerType` for CRDT metadata.
///
/// This enables `LwwRegister<T>` to report the correct `InnerType` based on `T`.
pub trait AsInnerType {
    /// Returns the `InnerType` that describes this type's Borsh serialization format.
    fn as_inner_type() -> InnerType;
}

// Implement AsInnerType for primitive types
impl AsInnerType for u8 {
    fn as_inner_type() -> InnerType {
        InnerType::U8
    }
}
impl AsInnerType for u16 {
    fn as_inner_type() -> InnerType {
        InnerType::U16
    }
}
impl AsInnerType for u32 {
    fn as_inner_type() -> InnerType {
        InnerType::U32
    }
}
impl AsInnerType for u64 {
    fn as_inner_type() -> InnerType {
        InnerType::U64
    }
}
impl AsInnerType for u128 {
    fn as_inner_type() -> InnerType {
        InnerType::U128
    }
}
impl AsInnerType for i8 {
    fn as_inner_type() -> InnerType {
        InnerType::I8
    }
}
impl AsInnerType for i16 {
    fn as_inner_type() -> InnerType {
        InnerType::I16
    }
}
impl AsInnerType for i32 {
    fn as_inner_type() -> InnerType {
        InnerType::I32
    }
}
impl AsInnerType for i64 {
    fn as_inner_type() -> InnerType {
        InnerType::I64
    }
}
impl AsInnerType for i128 {
    fn as_inner_type() -> InnerType {
        InnerType::I128
    }
}
impl AsInnerType for f32 {
    fn as_inner_type() -> InnerType {
        InnerType::F32
    }
}
impl AsInnerType for f64 {
    fn as_inner_type() -> InnerType {
        InnerType::F64
    }
}
impl AsInnerType for bool {
    fn as_inner_type() -> InnerType {
        InnerType::Bool
    }
}
impl AsInnerType for String {
    fn as_inner_type() -> InnerType {
        InnerType::String
    }
}
impl AsInnerType for Vec<u8> {
    fn as_inner_type() -> InnerType {
        InnerType::Bytes
    }
}

/// Identifies the specific CRDT type for merge dispatch during state synchronization.
///
/// # ID Assignment
///
/// Collections get deterministic IDs via the `#[app::state]` macro which calls
/// `reassign_deterministic_id(field_name)` after `init()` returns. This ensures:
///
/// - **CIP Invariant I9**: Given the same application code and field names, all nodes
///   generate identical entity IDs for the same logical entities.
///
/// # Merge Behavior
///
/// During synchronization, the storage layer uses `crdt_type` to dispatch merges:
///
/// - **Built-in types** (Counter, Map, Vector, etc.) - merged in the storage layer
/// - **Custom types** - dispatched to WASM for app-defined merge logic
/// - **None/Unknown** - falls back to Last-Write-Wins (LWW) semantics
///
/// # Nested Collections
///
/// For nested collections like `Map<String, Counter>`, the parent map stores
/// entries by key. The nested Counter's ID doesn't affect sync - merging happens
/// by the parent map's key. This is why nested collections can use `new()` with
/// random IDs while top-level fields need deterministic IDs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub enum CrdtType {
    /// Last-Write-Wins Register - wraps primitive types with timestamp-based conflict resolution.
    ///
    /// Merge: Higher timestamp wins, with node ID as tie-breaker.
    ///
    /// The `inner` field specifies the type of the wrapped value, enabling proper
    /// deserialization during merge. Use `InnerType::Custom` for app-defined types
    /// that require WASM callback for merging.
    LwwRegister {
        /// The type of the inner value (e.g., `InnerType::U64` for `LwwRegister<u64>`)
        inner: InnerType,
    },

    /// PN-Counter - supports both increment and decrement operations.
    ///
    /// Internally uses two maps: positive and negative counts per executor.
    /// Merge: Union of positive maps, union of negative maps, then compute difference.
    /// Use `GCounter` if you only need increment operations.
    Counter,

    /// G-Counter - grow-only counter (increment only, no decrement).
    ///
    /// Internally uses a single map of positive counts per executor.
    /// Merge: Take max count per executor.
    /// More efficient than `Counter` (PNCounter) when decrement is not needed.
    GCounter,

    /// Replicated Growable Array - CRDT for collaborative text editing.
    ///
    /// Supports concurrent insertions and deletions with causal ordering.
    /// Merge: Interleave characters by (timestamp, node_id) ordering.
    Rga,

    /// Unordered Map - key-value store with add-wins semantics for keys.
    ///
    /// Keys are never lost once added (tombstoned but retained).
    /// Values are merged recursively if they implement Mergeable.
    /// Merge: Union of keys, recursive merge of values.
    UnorderedMap,

    /// Unordered Set - collection of unique values with add-wins semantics.
    ///
    /// Elements are never lost once added.
    /// Merge: Union of all elements from both sets.
    UnorderedSet,

    /// Vector - ordered list with append operations.
    ///
    /// Elements are identified by index + timestamp for ordering.
    /// Merge: Interleave by timestamp, preserving causal order.
    Vector,

    /// User Storage - per-user data storage with signature-based access control.
    ///
    /// Only the owning user (identified by executor ID) can modify their data.
    /// Merge: Latest update per user based on nonce/timestamp.
    UserStorage,

    /// Frozen Storage - write-once storage for immutable data.
    ///
    /// Data can be written once and never modified or deleted.
    /// Merge: First-write-wins (subsequent writes are no-ops).
    FrozenStorage,

    /// Record - a composite struct that merges field-by-field.
    ///
    /// Used for the root application state (annotated with `#[app::state]`).
    /// Each field is merged according to its own CRDT type.
    /// Merge: Recursively merge each field using the auto-generated `Mergeable` impl.
    Record,

    /// Custom user-defined CRDT type.
    ///
    /// For types annotated with `#[derive(CrdtState)]` that define custom merge logic.
    /// Merge: Dispatched to WASM runtime to call the app's merge function.
    Custom(String),
}

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

/// Errors that can occur during CRDT merging
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    /// Incompatible CRDT states (shouldn't happen in practice)
    IncompatibleStates,
    /// Storage error during merge
    StorageError(String),
    /// Type mismatch (attempted to merge different CRDT types)
    TypeMismatch,
    /// CRDT type requires WASM execution for merge (custom app-defined types)
    WasmRequired {
        /// Name of the custom type that requires WASM merge
        type_name: String,
    },
    /// Serialization or deserialization error during merge
    SerializationError(String),
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::IncompatibleStates => write!(f, "Incompatible CRDT states"),
            MergeError::StorageError(msg) => write!(f, "Storage error: {}", msg),
            MergeError::TypeMismatch => write!(f, "Cannot merge different CRDT types"),
            MergeError::WasmRequired { type_name } => {
                write!(f, "CRDT type '{}' requires WASM for merge", type_name)
            }
            MergeError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
        }
    }
}

impl std::error::Error for MergeError {}

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
