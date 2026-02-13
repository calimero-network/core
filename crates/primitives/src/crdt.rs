//! CRDT (Conflict-free Replicated Data Type) primitives.
//!
//! This module provides the unified `CrdtType` enum used across the codebase
//! for identifying CRDT semantics during storage and synchronization.

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// CRDT type indicator for merge semantics.
///
/// Identifies the conflict resolution strategy used when merging replicated data.
/// This enum is used both by the storage layer (for persistence metadata) and
/// the sync protocol (for wire-format entity classification).
///
/// # Merge Semantics
///
/// Each variant defines specific merge behavior:
/// - **Registers**: LwwRegister (timestamp-based)
/// - **Counters**: GCounter (grow-only), PnCounter (increment/decrement)
/// - **Collections**: Rga, UnorderedMap, UnorderedSet, Vector
/// - **Special**: UserStorage, FrozenStorage, Custom
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub enum CrdtType {
    // =========================================================================
    // REGISTERS
    // =========================================================================
    /// Last-Writer-Wins Register.
    ///
    /// Wraps primitive types with timestamp-based conflict resolution.
    /// Merge: Higher HLC timestamp wins, with node ID as tie-breaker.
    ///
    /// The inner type name enables proper deserialization during merge.
    LwwRegister {
        /// Inner type name (e.g., "String", "u64", "MyStruct")
        inner_type: String,
    },

    // =========================================================================
    // COUNTERS
    // =========================================================================
    /// Grow-only Counter.
    ///
    /// Supports only increment operations; value can never decrease.
    /// Internally tracks increments per executor.
    /// Merge: Take maximum of each executor's count.
    GCounter,

    /// Positive-Negative Counter.
    ///
    /// Supports both increment and decrement operations.
    /// Internally uses two maps: positive and negative counts per executor.
    /// Merge: Union of positive maps, union of negative maps, then compute difference.
    PnCounter,

    // =========================================================================
    // COLLECTIONS
    // =========================================================================
    /// Replicated Growable Array.
    ///
    /// CRDT for collaborative text editing and ordered sequences.
    /// Supports concurrent insertions and deletions with causal ordering.
    /// Merge: Interleave elements by (timestamp, node_id) ordering.
    Rga,

    /// Unordered Map.
    ///
    /// Key-value store with add-wins semantics for keys.
    /// Keys are never lost once added (tombstoned but retained).
    /// Values are merged recursively if they implement Mergeable.
    /// Merge: Union of keys, recursive merge of values.
    UnorderedMap {
        /// Key type name
        key_type: String,
        /// Value type name (may be a nested CRDT)
        value_type: String,
    },

    /// Unordered Set.
    ///
    /// Collection of unique values with add-wins semantics.
    /// Elements are never lost once added.
    /// Merge: Union of all elements from both sets.
    UnorderedSet {
        /// Element type name
        element_type: String,
    },

    /// Vector (ordered collection).
    ///
    /// Ordered list with append operations.
    /// Elements are identified by index + timestamp for ordering.
    /// Merge: Element-wise merge by index with timestamp ordering.
    Vector {
        /// Element type name (may be a nested CRDT)
        element_type: String,
    },

    // =========================================================================
    // SPECIAL STORAGE
    // =========================================================================
    /// User Storage.
    ///
    /// Per-user data storage with signature-based access control.
    /// Only the owning user (identified by executor ID) can modify their data.
    /// Merge: Latest update per user based on nonce/timestamp.
    UserStorage,

    /// Frozen Storage.
    ///
    /// Write-once storage for immutable data.
    /// Data can be written once and never modified or deleted.
    /// Merge: First-write-wins (subsequent writes are no-ops).
    FrozenStorage,

    /// Custom CRDT with app-defined merge.
    ///
    /// For types annotated with `#[derive(CrdtState)]` that define custom merge logic.
    /// The string identifies the custom type name within the application.
    /// Merge: Dispatched to WASM runtime to call the app's merge function.
    Custom(String),
}

impl Default for CrdtType {
    fn default() -> Self {
        Self::LwwRegister {
            inner_type: String::new(),
        }
    }
}

impl CrdtType {
    /// Create an LwwRegister with a known inner type.
    #[must_use]
    pub fn lww_register(inner_type: impl Into<String>) -> Self {
        Self::LwwRegister {
            inner_type: inner_type.into(),
        }
    }

    /// Create an UnorderedMap with known key and value types.
    #[must_use]
    pub fn unordered_map(key_type: impl Into<String>, value_type: impl Into<String>) -> Self {
        Self::UnorderedMap {
            key_type: key_type.into(),
            value_type: value_type.into(),
        }
    }

    /// Create an UnorderedSet with a known element type.
    #[must_use]
    pub fn unordered_set(element_type: impl Into<String>) -> Self {
        Self::UnorderedSet {
            element_type: element_type.into(),
        }
    }

    /// Create a Vector with a known element type.
    #[must_use]
    pub fn vector(element_type: impl Into<String>) -> Self {
        Self::Vector {
            element_type: element_type.into(),
        }
    }

    /// Returns `true` if this is a counter type (GCounter or PnCounter).
    #[must_use]
    pub const fn is_counter(&self) -> bool {
        matches!(self, Self::GCounter | Self::PnCounter)
    }

    /// Returns `true` if this is a set type.
    #[must_use]
    pub const fn is_set(&self) -> bool {
        matches!(self, Self::UnorderedSet { .. })
    }

    /// Returns `true` if this is a collection type (map, set, vector, or array).
    #[must_use]
    pub const fn is_collection(&self) -> bool {
        matches!(
            self,
            Self::UnorderedMap { .. } | Self::UnorderedSet { .. } | Self::Vector { .. } | Self::Rga
        )
    }

    /// Returns `true` if this is a custom CRDT type.
    #[must_use]
    pub const fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    /// Returns `true` if this type requires special storage handling.
    #[must_use]
    pub const fn is_special_storage(&self) -> bool {
        matches!(self, Self::UserStorage | Self::FrozenStorage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_lww_register() {
        assert!(matches!(CrdtType::default(), CrdtType::LwwRegister { .. }));
    }

    #[test]
    fn test_lww_register_constructor() {
        let lww = CrdtType::lww_register("String");
        assert_eq!(
            lww,
            CrdtType::LwwRegister {
                inner_type: "String".to_string()
            }
        );
    }

    #[test]
    fn test_is_counter() {
        assert!(CrdtType::GCounter.is_counter());
        assert!(CrdtType::PnCounter.is_counter());
        assert!(!CrdtType::lww_register("u64").is_counter());
        assert!(!CrdtType::unordered_map("String", "u64").is_counter());
    }

    #[test]
    fn test_is_set() {
        assert!(CrdtType::unordered_set("String").is_set());
        assert!(!CrdtType::unordered_map("String", "u64").is_set());
        assert!(!CrdtType::vector("u64").is_set());
    }

    #[test]
    fn test_is_collection() {
        assert!(CrdtType::unordered_map("String", "u64").is_collection());
        assert!(CrdtType::unordered_set("String").is_collection());
        assert!(CrdtType::vector("u64").is_collection());
        assert!(CrdtType::Rga.is_collection());
        assert!(!CrdtType::lww_register("u64").is_collection());
        assert!(!CrdtType::GCounter.is_collection());
        assert!(!CrdtType::PnCounter.is_collection());
    }

    #[test]
    fn test_is_custom() {
        assert!(CrdtType::Custom("test".to_string()).is_custom());
        assert!(!CrdtType::lww_register("u64").is_custom());
    }

    #[test]
    fn test_is_special_storage() {
        assert!(CrdtType::UserStorage.is_special_storage());
        assert!(CrdtType::FrozenStorage.is_special_storage());
        assert!(!CrdtType::lww_register("u64").is_special_storage());
        assert!(!CrdtType::GCounter.is_special_storage());
    }

    #[test]
    fn test_serde_roundtrip() {
        let types = [
            CrdtType::lww_register("String"),
            CrdtType::lww_register("u64"),
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::Rga,
            CrdtType::unordered_map("String", "u64"),
            CrdtType::unordered_set("String"),
            CrdtType::vector("u64"),
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::Custom("my_type".to_string()),
        ];

        for crdt_type in &types {
            let json = serde_json::to_string(crdt_type).unwrap();
            let decoded: CrdtType = serde_json::from_str(&json).unwrap();
            assert_eq!(*crdt_type, decoded);
        }
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn test_borsh_roundtrip() {
        let types = [
            CrdtType::lww_register("String"),
            CrdtType::lww_register("u64"),
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::Rga,
            CrdtType::unordered_map("String", "u64"),
            CrdtType::unordered_set("String"),
            CrdtType::vector("u64"),
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::Custom("my_type".to_string()),
        ];

        for crdt_type in &types {
            let bytes = borsh::to_vec(crdt_type).unwrap();
            let decoded: CrdtType = borsh::from_slice(&bytes).unwrap();
            assert_eq!(*crdt_type, decoded);
        }
    }
}
