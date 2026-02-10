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
/// # Wire Protocol
///
/// When transmitted over the network, this enum uses Borsh serialization.
/// The `Custom` variant uses a `u32` discriminator for efficient encoding.
///
/// # Borsh Discriminants
///
/// Explicit discriminants are used to maintain backward compatibility with the
/// sync wire protocol. The order matches the original sync `CrdtType` enum:
/// - Variants 0-8: Core CRDT types (sync protocol compatible)
/// - Variant 9: Custom (sync protocol compatible)
/// - Variants 10-12: Storage-only types (not sent over wire)
///
/// # Merge Semantics
///
/// Each variant defines specific merge behavior:
/// - **State-based**: LwwRegister, GCounter, PnCounter, LwwSet, OrSet
/// - **Operation-based**: Rga, Vector
/// - **Composite**: UnorderedMap, UnorderedSet, Record
/// - **Special**: UserStorage, FrozenStorage, Custom
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[non_exhaustive]
pub enum CrdtType {
    /// Last-Writer-Wins Register.
    ///
    /// Wraps primitive types with timestamp-based conflict resolution.
    /// Merge: Higher HLC timestamp wins, with node ID as tie-breaker.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 0))]
    LwwRegister,

    /// Grow-only Counter.
    ///
    /// Supports only increment operations; value can never decrease.
    /// Merge: Take maximum of each node's count.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 1))]
    GCounter,

    /// Positive-Negative Counter.
    ///
    /// Supports both increment and decrement operations.
    /// Internally uses two maps: positive and negative counts per executor.
    /// Merge: Union of positive maps, union of negative maps, then compute difference.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 2))]
    PnCounter,

    /// Last-Writer-Wins Element Set.
    ///
    /// Set where each element has an associated timestamp.
    /// Merge: Per-element timestamp comparison; latest operation (add/remove) wins.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 3))]
    LwwSet,

    /// Observed-Remove Set.
    ///
    /// Set with add-wins semantics and causal remove tracking.
    /// Merge: Union of adds, respecting remove tombstones with causal ordering.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 4))]
    OrSet,

    /// Replicated Growable Array.
    ///
    /// CRDT for collaborative text editing and ordered sequences.
    /// Supports concurrent insertions and deletions with causal ordering.
    /// Merge: Interleave elements by (timestamp, node_id) ordering.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 5))]
    Rga,

    /// Unordered Map.
    ///
    /// Key-value store with add-wins semantics for keys.
    /// Keys are never lost once added (tombstoned but retained).
    /// Values are merged recursively if they implement Mergeable.
    /// Merge: Union of keys, recursive merge of values.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 6))]
    UnorderedMap,

    /// Unordered Set.
    ///
    /// Collection of unique values with add-wins semantics.
    /// Elements are never lost once added.
    /// Merge: Union of all elements from both sets.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 7))]
    UnorderedSet,

    /// Vector (ordered collection).
    ///
    /// Ordered list with append operations.
    /// Elements are identified by index + timestamp for ordering.
    /// Merge: Element-wise merge by index with timestamp ordering.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 8))]
    Vector,

    /// Custom CRDT with app-defined merge.
    ///
    /// For types annotated with `#[derive(CrdtState)]` that define custom merge logic.
    /// The `u32` discriminator identifies the custom type within the application.
    /// Merge: Dispatched to WASM runtime to call the app's merge function.
    ///
    /// Note: Placed at discriminant 9 for sync wire protocol backward compatibility.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 9))]
    Custom(u32),

    /// User Storage.
    ///
    /// Per-user data storage with signature-based access control.
    /// Only the owning user (identified by executor ID) can modify their data.
    /// Merge: Latest update per user based on nonce/timestamp.
    ///
    /// Note: Storage-only type, not transmitted over sync wire protocol.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 10))]
    UserStorage,

    /// Frozen Storage.
    ///
    /// Write-once storage for immutable data.
    /// Data can be written once and never modified or deleted.
    /// Merge: First-write-wins (subsequent writes are no-ops).
    ///
    /// Note: Storage-only type, not transmitted over sync wire protocol.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 11))]
    FrozenStorage,

    /// Record (composite struct).
    ///
    /// Used for the root application state (annotated with `#[app::state]`).
    /// Each field is merged according to its own CRDT type.
    /// Merge: Recursively merge each field using the auto-generated `Mergeable` impl.
    ///
    /// Note: Storage-only type, not transmitted over sync wire protocol.
    #[cfg_attr(feature = "borsh", borsh(discriminant = 12))]
    Record,
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
        assert!(!CrdtType::GCounter.is_collection());
    }

    #[test]
    fn test_is_custom() {
        assert!(CrdtType::Custom(0).is_custom());
        assert!(CrdtType::Custom(42).is_custom());
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
    fn test_ordering() {
        // Verify enum variants have a defined ordering (useful for storage)
        assert!(CrdtType::LwwRegister < CrdtType::GCounter);
        assert!(CrdtType::GCounter < CrdtType::PnCounter);
        assert!(CrdtType::Custom(0) < CrdtType::Custom(1));
    }

    #[test]
    fn test_serde_roundtrip() {
        let types = [
            CrdtType::LwwRegister,
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::LwwSet,
            CrdtType::OrSet,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::UnorderedSet,
            CrdtType::Vector,
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::Record,
            CrdtType::Custom(42),
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
            CrdtType::LwwSet,
            CrdtType::OrSet,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::UnorderedSet,
            CrdtType::Vector,
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::Record,
            CrdtType::Custom(42),
        ];

        for crdt_type in &types {
            let bytes = borsh::to_vec(crdt_type).unwrap();
            let decoded: CrdtType = borsh::from_slice(&bytes).unwrap();
            assert_eq!(*crdt_type, decoded);
        }
    }
}
