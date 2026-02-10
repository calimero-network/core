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
///
/// # Borsh Discriminants
///
/// Explicit discriminants are used for backward compatibility with persisted data.
/// The old storage `CrdtType` had `Counter` (now `PnCounter`) at discriminant 1.
/// New variants like `GCounter` use discriminant 10+ to avoid conflicts.
/// **Do not change these discriminant values** without a data migration strategy.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(
    feature = "borsh",
    derive(BorshSerialize, BorshDeserialize),
    borsh(use_discriminant = true)
)]
#[non_exhaustive]
#[repr(u8)]
pub enum CrdtType {
    // =========================================================================
    // REGISTERS
    // =========================================================================
    /// Last-Writer-Wins Register.
    ///
    /// Wraps primitive types with timestamp-based conflict resolution.
    /// Merge: Higher HLC timestamp wins, with node ID as tie-breaker.
    LwwRegister = 0,

    // =========================================================================
    // COUNTERS
    // =========================================================================
    /// Positive-Negative Counter.
    ///
    /// Supports both increment and decrement operations.
    /// Internally uses two maps: positive and negative counts per executor.
    /// Merge: Union of positive maps, union of negative maps, then compute difference.
    ///
    /// Note: Uses discriminant 1 for backward compatibility with the old `Counter` variant.
    PnCounter = 1,

    // =========================================================================
    // COLLECTIONS
    // =========================================================================
    /// Replicated Growable Array.
    ///
    /// CRDT for collaborative text editing and ordered sequences.
    /// Supports concurrent insertions and deletions with causal ordering.
    /// Merge: Interleave elements by (timestamp, node_id) ordering.
    Rga = 2,

    /// Unordered Map.
    ///
    /// Key-value store with add-wins semantics for keys.
    /// Keys are never lost once added (tombstoned but retained).
    /// Values are merged recursively if they implement Mergeable.
    /// Merge: Union of keys, recursive merge of values.
    UnorderedMap = 3,

    /// Unordered Set.
    ///
    /// Collection of unique values with add-wins semantics.
    /// Elements are never lost once added.
    /// Merge: Union of all elements from both sets.
    UnorderedSet = 4,

    /// Vector (ordered collection).
    ///
    /// Ordered list with append operations.
    /// Elements are identified by index + timestamp for ordering.
    /// Merge: Element-wise merge by index with timestamp ordering.
    Vector = 5,

    // =========================================================================
    // SPECIAL STORAGE
    // =========================================================================
    /// User Storage.
    ///
    /// Per-user data storage with signature-based access control.
    /// Only the owning user (identified by executor ID) can modify their data.
    /// Merge: Latest update per user based on nonce/timestamp.
    UserStorage = 6,

    /// Frozen Storage.
    ///
    /// Write-once storage for immutable data.
    /// Data can be written once and never modified or deleted.
    /// Merge: First-write-wins (subsequent writes are no-ops).
    FrozenStorage = 7,

    // Discriminant 8 was previously used by `Record` variant (now removed).
    // Do not reuse discriminant 8 to avoid deserialization conflicts with old data.
    /// Custom CRDT with app-defined merge.
    ///
    /// For types annotated with `#[derive(CrdtState)]` that define custom merge logic.
    /// The string identifies the custom type name within the application.
    /// Merge: Dispatched to WASM runtime to call the app's merge function.
    Custom(String) = 9,

    /// Grow-only Counter.
    ///
    /// Supports only increment operations; value can never decrease.
    /// Internally tracks increments per executor.
    /// Merge: Take maximum of each executor's count.
    ///
    /// Note: Uses discriminant 10 as this is a new variant not present in old storage format.
    GCounter = 10,
}

impl Default for CrdtType {
    fn default() -> Self {
        Self::LwwRegister
    }
}

impl CrdtType {
    /// Returns `true` if this is a counter type (GCounter or PnCounter).
    #[must_use]
    pub const fn is_counter(&self) -> bool {
        matches!(self, Self::GCounter | Self::PnCounter)
    }

    /// Returns `true` if this is a set type.
    #[must_use]
    pub const fn is_set(&self) -> bool {
        matches!(self, Self::UnorderedSet)
    }

    /// Returns `true` if this is a collection type (map, set, vector, or array).
    #[must_use]
    pub const fn is_collection(&self) -> bool {
        matches!(
            self,
            Self::UnorderedMap | Self::UnorderedSet | Self::Vector | Self::Rga
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
        assert_eq!(CrdtType::default(), CrdtType::LwwRegister);
    }

    #[test]
    fn test_is_counter() {
        assert!(CrdtType::GCounter.is_counter());
        assert!(CrdtType::PnCounter.is_counter());
        assert!(!CrdtType::LwwRegister.is_counter());
        assert!(!CrdtType::UnorderedMap.is_counter());
    }

    #[test]
    fn test_is_set() {
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
        assert!(!CrdtType::LwwRegister.is_collection());
        assert!(!CrdtType::GCounter.is_collection());
        assert!(!CrdtType::PnCounter.is_collection());
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
        assert!(!CrdtType::GCounter.is_special_storage());
    }

    #[test]
    fn test_serde_roundtrip() {
        let types = [
            CrdtType::LwwRegister,
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::UnorderedSet,
            CrdtType::Vector,
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
            CrdtType::LwwRegister,
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::UnorderedSet,
            CrdtType::Vector,
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

    #[cfg(feature = "borsh")]
    #[test]
    fn test_borsh_discriminants_backward_compatible() {
        // Verify explicit discriminant values match the old storage format.
        // These values MUST NOT change to maintain backward compatibility with persisted data.
        //
        // Old storage CrdtType discriminants:
        //   LwwRegister = 0, Counter = 1, Rga = 2, UnorderedMap = 3,
        //   UnorderedSet = 4, Vector = 5, UserStorage = 6, FrozenStorage = 7,
        //   Record = 8 (removed), Custom = 9
        //
        // New mapping:
        //   PnCounter = 1 (replaces old Counter which was semantically a PN-Counter)
        //   GCounter = 10 (new variant, uses new discriminant)

        let test_cases: &[(CrdtType, u8)] = &[
            (CrdtType::LwwRegister, 0),
            (CrdtType::PnCounter, 1), // Was 'Counter' in old format
            (CrdtType::Rga, 2),
            (CrdtType::UnorderedMap, 3),
            (CrdtType::UnorderedSet, 4),
            (CrdtType::Vector, 5),
            (CrdtType::UserStorage, 6),
            (CrdtType::FrozenStorage, 7),
            // Discriminant 8 was 'Record', now removed
            (CrdtType::Custom("".to_string()), 9),
            (CrdtType::GCounter, 10), // New variant
        ];

        for (variant, expected_discriminant) in test_cases {
            let bytes = borsh::to_vec(variant).unwrap();
            assert_eq!(
                bytes[0], *expected_discriminant,
                "Discriminant mismatch for {:?}: expected {}, got {}",
                variant, expected_discriminant, bytes[0]
            );
        }
    }
}
