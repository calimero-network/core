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
/// # Backward Compatibility
///
/// **IMPORTANT**: The variant order is part of the Borsh serialization format.
/// Existing variants MUST NOT be reordered. New variants should be added at the end.
///
/// # Merge Semantics
///
/// Each variant defines specific merge behavior:
/// - **State-based**: LwwRegister, Counter, LwwSet, OrSet
/// - **Operation-based**: Rga, Vector
/// - **Composite**: UnorderedMap, UnorderedSet, Record
/// - **Special**: UserStorage, FrozenStorage, Custom
// NOTE: Variant order is stable for Borsh compatibility. Do NOT reorder existing variants.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[non_exhaustive]
pub enum CrdtType {
    // =========================================================================
    // STABLE VARIANTS (from original storage enum) - DO NOT REORDER
    // =========================================================================
    /// Last-Writer-Wins Register.
    ///
    /// Wraps primitive types with timestamp-based conflict resolution.
    /// Merge: Higher HLC timestamp wins, with node ID as tie-breaker.
    LwwRegister, // discriminant 0

    /// Counter (PN-Counter semantics).
    ///
    /// Supports both increment and decrement operations.
    /// Internally uses two maps: positive and negative counts per executor.
    /// Merge: Union of positive maps, union of negative maps, then compute difference.
    ///
    /// Note: Both GCounter and PNCounter types report this variant for backward
    /// compatibility. Use the collection type itself to distinguish grow-only
    /// vs positive-negative semantics.
    Counter, // discriminant 1

    /// Replicated Growable Array.
    ///
    /// CRDT for collaborative text editing and ordered sequences.
    /// Supports concurrent insertions and deletions with causal ordering.
    /// Merge: Interleave elements by (timestamp, node_id) ordering.
    Rga, // discriminant 2

    /// Unordered Map.
    ///
    /// Key-value store with add-wins semantics for keys.
    /// Keys are never lost once added (tombstoned but retained).
    /// Values are merged recursively if they implement Mergeable.
    /// Merge: Union of keys, recursive merge of values.
    UnorderedMap, // discriminant 3

    /// Unordered Set.
    ///
    /// Collection of unique values with add-wins semantics.
    /// Elements are never lost once added.
    /// Merge: Union of all elements from both sets.
    UnorderedSet, // discriminant 4

    /// Vector (ordered collection).
    ///
    /// Ordered list with append operations.
    /// Elements are identified by index + timestamp for ordering.
    /// Merge: Element-wise merge by index with timestamp ordering.
    Vector, // discriminant 5

    /// User Storage.
    ///
    /// Per-user data storage with signature-based access control.
    /// Only the owning user (identified by executor ID) can modify their data.
    /// Merge: Latest update per user based on nonce/timestamp.
    UserStorage, // discriminant 6

    /// Frozen Storage.
    ///
    /// Write-once storage for immutable data.
    /// Data can be written once and never modified or deleted.
    /// Merge: First-write-wins (subsequent writes are no-ops).
    FrozenStorage, // discriminant 7

    /// Record (composite struct).
    ///
    /// Used for the root application state (annotated with `#[app::state]`).
    /// Each field is merged according to its own CRDT type.
    /// Merge: Recursively merge each field using the auto-generated `Mergeable` impl.
    Record, // discriminant 8

    /// Custom CRDT with app-defined merge.
    ///
    /// For types annotated with `#[derive(CrdtState)]` that define custom merge logic.
    /// The string identifies the custom type name within the application.
    /// Merge: Dispatched to WASM runtime to call the app's merge function.
    Custom(String), // discriminant 9

    // =========================================================================
    // NEW VARIANTS (added for sync protocol) - safe to add at end
    // =========================================================================
    /// Last-Writer-Wins Element Set.
    ///
    /// Set where each element has an associated timestamp.
    /// Merge: Per-element timestamp comparison; latest operation (add/remove) wins.
    LwwSet, // discriminant 10

    /// Observed-Remove Set.
    ///
    /// Set with add-wins semantics and causal remove tracking.
    /// Merge: Union of adds, respecting remove tombstones with causal ordering.
    OrSet, // discriminant 11
}

impl Default for CrdtType {
    fn default() -> Self {
        Self::LwwRegister
    }
}

impl CrdtType {
    /// Returns `true` if this is a counter type.
    #[must_use]
    pub const fn is_counter(&self) -> bool {
        matches!(self, Self::Counter)
    }

    /// Returns `true` if this is a set type (LwwSet, OrSet, or UnorderedSet).
    #[must_use]
    pub const fn is_set(&self) -> bool {
        matches!(self, Self::LwwSet | Self::OrSet | Self::UnorderedSet)
    }

    /// Returns `true` if this is a collection type (map, set, vector, or array).
    #[must_use]
    pub const fn is_collection(&self) -> bool {
        matches!(
            self,
            Self::UnorderedMap
                | Self::UnorderedSet
                | Self::Vector
                | Self::Rga
                | Self::LwwSet
                | Self::OrSet
        )
    }

    /// Returns `true` if this is a custom CRDT type.
    #[must_use]
    pub fn is_custom(&self) -> bool {
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
        assert_eq!(CrdtType::default(), CrdtType::LwwRegister);
    }

    #[test]
    fn test_is_counter() {
        assert!(CrdtType::Counter.is_counter());
        assert!(!CrdtType::LwwRegister.is_counter());
        assert!(!CrdtType::UnorderedMap.is_counter());
    }

    #[test]
    fn test_is_set() {
        assert!(CrdtType::LwwSet.is_set());
        assert!(CrdtType::OrSet.is_set());
        assert!(CrdtType::UnorderedSet.is_set());
        assert!(!CrdtType::UnorderedMap.is_set());
        assert!(!CrdtType::Vector.is_set());
    }

    #[test]
    fn test_is_collection() {
        assert!(CrdtType::UnorderedMap.is_collection());
        assert!(CrdtType::UnorderedSet.is_collection());
        assert!(CrdtType::Vector.is_collection());
        assert!(CrdtType::Rga.is_collection());
        assert!(CrdtType::LwwSet.is_collection());
        assert!(CrdtType::OrSet.is_collection());
        assert!(!CrdtType::LwwRegister.is_collection());
        assert!(!CrdtType::Counter.is_collection());
    }

    #[test]
    fn test_is_custom() {
        assert!(CrdtType::Custom("test".to_string()).is_custom());
        assert!(!CrdtType::LwwRegister.is_custom());
    }

    #[test]
    fn test_is_special_storage() {
        assert!(CrdtType::UserStorage.is_special_storage());
        assert!(CrdtType::FrozenStorage.is_special_storage());
        assert!(!CrdtType::LwwRegister.is_special_storage());
        assert!(!CrdtType::Record.is_special_storage());
    }

    #[test]
    fn test_serde_roundtrip() {
        let types = [
            CrdtType::LwwRegister,
            CrdtType::Counter,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::UnorderedSet,
            CrdtType::Vector,
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::Record,
            CrdtType::Custom("my_type".to_string()),
            CrdtType::LwwSet,
            CrdtType::OrSet,
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
            CrdtType::LwwRegister,
            CrdtType::Counter,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::UnorderedSet,
            CrdtType::Vector,
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::Record,
            CrdtType::Custom("my_type".to_string()),
            CrdtType::LwwSet,
            CrdtType::OrSet,
        ];

        for crdt_type in &types {
            let bytes = borsh::to_vec(crdt_type).unwrap();
            let decoded: CrdtType = borsh::from_slice(&bytes).unwrap();
            assert_eq!(*crdt_type, decoded);
        }
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn test_borsh_discriminants_stable() {
        // Verify discriminants match expected values for backward compatibility
        // These values MUST NOT change or existing data will fail to deserialize
        assert_eq!(borsh::to_vec(&CrdtType::LwwRegister).unwrap(), vec![0]);
        assert_eq!(borsh::to_vec(&CrdtType::Counter).unwrap(), vec![1]);
        assert_eq!(borsh::to_vec(&CrdtType::Rga).unwrap(), vec![2]);
        assert_eq!(borsh::to_vec(&CrdtType::UnorderedMap).unwrap(), vec![3]);
        assert_eq!(borsh::to_vec(&CrdtType::UnorderedSet).unwrap(), vec![4]);
        assert_eq!(borsh::to_vec(&CrdtType::Vector).unwrap(), vec![5]);
        assert_eq!(borsh::to_vec(&CrdtType::UserStorage).unwrap(), vec![6]);
        assert_eq!(borsh::to_vec(&CrdtType::FrozenStorage).unwrap(), vec![7]);
        assert_eq!(borsh::to_vec(&CrdtType::Record).unwrap(), vec![8]);
        // Custom(String) has discriminant 9 + string length + string bytes
        let custom_bytes = borsh::to_vec(&CrdtType::Custom("x".to_string())).unwrap();
        assert_eq!(custom_bytes[0], 9); // discriminant
                                        // New variants at the end
        assert_eq!(borsh::to_vec(&CrdtType::LwwSet).unwrap(), vec![10]);
        assert_eq!(borsh::to_vec(&CrdtType::OrSet).unwrap(), vec![11]);
    }
}
