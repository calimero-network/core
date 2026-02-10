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
/// Discriminants are explicitly assigned for Borsh serialization backward compatibility.
/// The old `calimero_storage::collections::crdt_meta::CrdtType` enum had:
/// - LwwRegister=0, Counter=1, Rga=2, UnorderedMap=3, UnorderedSet=4, Vector=5,
/// - UserStorage=6, FrozenStorage=7, Record=8, Custom=9
/// New variants must use discriminants >= 10 to avoid conflicts.
/// Note: `Counter` was renamed to `GCounter`; `Record` (discriminant 8) was removed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "borsh", borsh(use_discriminant = true))]
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
    /// Grow-only Counter.
    ///
    /// Supports only increment operations; value can never decrease.
    /// Internally tracks increments per executor.
    /// Merge: Take maximum of each executor's count.
    GCounter = 1,

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

    // Note: discriminant 8 was `Record` in the old enum, now removed.
    /// Custom CRDT with app-defined merge.
    ///
    /// For types annotated with `#[derive(CrdtState)]` that define custom merge logic.
    /// The string identifies the custom type name within the application.
    /// Merge: Dispatched to WASM runtime to call the app's merge function.
    Custom(String) = 9,

    // =========================================================================
    // NEW VARIANTS (discriminants >= 10)
    // =========================================================================
    /// Positive-Negative Counter.
    ///
    /// Supports both increment and decrement operations.
    /// Internally uses two maps: positive and negative counts per executor.
    /// Merge: Union of positive maps, union of negative maps, then compute difference.
    PnCounter = 10,
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

    /// Verifies Borsh discriminant values for backward compatibility.
    ///
    /// These values MUST match the old `calimero_storage::collections::crdt_meta::CrdtType`
    /// enum to ensure existing persisted data deserializes correctly.
    /// DO NOT change these discriminants without a migration strategy.
    #[cfg(feature = "borsh")]
    #[test]
    fn test_borsh_discriminant_backward_compatibility() {
        // The first byte of Borsh-serialized enum is the discriminant.
        // These values must match the old storage CrdtType enum for backward compatibility:
        // Old enum: LwwRegister=0, Counter=1, Rga=2, UnorderedMap=3, UnorderedSet=4,
        //           Vector=5, UserStorage=6, FrozenStorage=7, Record=8, Custom=9
        // Note: Counter was renamed to GCounter; Record (8) was removed; PnCounter is new (10).

        let test_cases: &[(CrdtType, u8)] = &[
            (CrdtType::LwwRegister, 0),
            (CrdtType::GCounter, 1), // Was "Counter" in old enum
            (CrdtType::Rga, 2),
            (CrdtType::UnorderedMap, 3),
            (CrdtType::UnorderedSet, 4),
            (CrdtType::Vector, 5),
            (CrdtType::UserStorage, 6),
            (CrdtType::FrozenStorage, 7),
            // Discriminant 8 was Record, now removed
            (CrdtType::Custom("test".to_string()), 9),
            (CrdtType::PnCounter, 10), // New variant
        ];

        for (crdt_type, expected_discriminant) in test_cases {
            let bytes = borsh::to_vec(crdt_type).unwrap();
            assert_eq!(
                bytes[0], *expected_discriminant,
                "Borsh discriminant mismatch for {:?}: expected {}, got {}. \
                 This would break backward compatibility with persisted data!",
                crdt_type, expected_discriminant, bytes[0]
            );
        }
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn test_borsh_deserialize_legacy_discriminants() {
        // Verify we can deserialize data written with the old enum discriminants.
        // This simulates reading persisted data from a database.

        // Old "Counter" (discriminant 1) should deserialize as GCounter
        let legacy_counter_bytes: &[u8] = &[1]; // Just the discriminant
        let decoded: CrdtType = borsh::from_slice(legacy_counter_bytes).unwrap();
        assert_eq!(decoded, CrdtType::GCounter);

        // Old Rga (discriminant 2) should still be Rga
        let legacy_rga_bytes: &[u8] = &[2];
        let decoded: CrdtType = borsh::from_slice(legacy_rga_bytes).unwrap();
        assert_eq!(decoded, CrdtType::Rga);

        // Verify each legacy discriminant maps to the correct variant
        let legacy_mappings: &[(u8, CrdtType)] = &[
            (0, CrdtType::LwwRegister),
            (1, CrdtType::GCounter),
            (2, CrdtType::Rga),
            (3, CrdtType::UnorderedMap),
            (4, CrdtType::UnorderedSet),
            (5, CrdtType::Vector),
            (6, CrdtType::UserStorage),
            (7, CrdtType::FrozenStorage),
        ];

        for (discriminant, expected) in legacy_mappings {
            let bytes: &[u8] = &[*discriminant];
            let decoded: CrdtType = borsh::from_slice(bytes).unwrap();
            assert_eq!(
                decoded, *expected,
                "Legacy discriminant {} should decode to {:?}",
                discriminant, expected
            );
        }
    }
}
