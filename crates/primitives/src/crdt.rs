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
    LwwRegister {
        /// Vestigial - written as `""`, read back only for the `"Opaque"` and
        /// [`JS_ROOT_CRDT_TYPE_NAME`] sentinels. Kept for borsh compatibility.
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
        /// Vestigial - always `""`; kept for borsh compatibility.
        key_type: String,
        /// Vestigial - always `""`; kept for borsh compatibility.
        value_type: String,
    },

    /// Sorted Map.
    ///
    /// Key-value store with the same add-wins merge semantics as
    /// [`UnorderedMap`](Self::UnorderedMap), but iterated in ascending key
    /// order to support range and prefix queries. The ordering is derived from
    /// `K: Ord` and is therefore a pure function of the key set — no extra
    /// state is synced and merge is byte-identical to `UnorderedMap`.
    /// Merge: Union of keys, recursive merge of values.
    SortedMap {
        /// Vestigial - always `""`; kept for borsh compatibility.
        key_type: String,
        /// Vestigial - always `""`; kept for borsh compatibility.
        value_type: String,
    },

    /// Unordered Set.
    ///
    /// Collection of unique values with add-wins semantics.
    /// Elements are never lost once added.
    /// Merge: Union of all elements from both sets.
    UnorderedSet {
        /// Vestigial - always `""`; kept for borsh compatibility.
        element_type: String,
    },

    /// Sorted Set.
    ///
    /// Same add-wins union merge as [`UnorderedSet`](Self::UnorderedSet), but
    /// iterated in ascending element order to support range and prefix queries.
    /// Ordering is derived from `T: Ord` — no extra state is synced.
    /// Merge: Union of all elements from both sets.
    SortedSet {
        /// Vestigial - always `""`; kept for borsh compatibility.
        element_type: String,
    },

    /// Vector (ordered collection).
    ///
    /// Ordered list with append operations.
    /// Elements are identified by index + timestamp for ordering.
    /// Merge: Element-wise merge by index with timestamp ordering.
    Vector {
        /// Vestigial - always `""`; kept for borsh compatibility.
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

    /// Shared Storage.
    ///
    /// Group-writable storage with a mutable writer set.
    /// Any key in the stored writer set can modify; rotation is signed by a current writer.
    /// Merge: Latest update per writer based on nonce/timestamp.
    SharedStorage,

    /// Custom CRDT with app-defined merge.
    ///
    /// For types annotated with `#[derive(CrdtState)]` that define custom merge logic.
    /// The string identifies the custom type name within the application.
    /// Merge: Dispatched to WASM runtime to call the app's merge function.
    Custom(String),

    /// Rotation log (P3 of core#2716).
    ///
    /// A per-`Shared`-anchor child (in a keyed collection) holding one writer-set
    /// rotation entry, stored as `borsh(RotationLog)`. Accumulation across
    /// `delta_id`s is structural (the collection's add-wins children); a
    /// same-`delta_id` collision resolves by LWW. Entries are authenticated at
    /// **resolve time** (each carries its signature + signed payload), not at
    /// merge time, so the merge itself trusts nothing.
    ///
    /// DECLARED LAST on purpose: borsh enums are discriminant-by-position, so a
    /// new variant must be appended — inserting it mid-enum shifts every later
    /// variant's tag and misaligns any `CrdtType` that crosses a serialization
    /// boundary (snapshots, persisted index metadata, mixed-binary peers),
    /// surfacing as `EntityIndex` borsh decode failures.
    RotationLog,
}

impl Default for CrdtType {
    fn default() -> Self {
        Self::LwwRegister {
            inner_type: String::new(),
        }
    }
}

/// Wire/persistence marker for a JS SDK app root that carries a guest-provided
/// `__calimero_merge_root_state` callback.
///
/// A JS app's root is not a `#[app::state]` type, so core has no registered
/// `Mergeable` for it and would otherwise treat the root as opaque
/// (`crdt_type: None`) and resolve conflicts by Last-Writer-Wins — which cannot
/// converge concurrent writers. When the guest calls `register_js_sdk_root_merge`,
/// the runtime stamps the root with `LwwRegister { inner_type: "JsRoot" }` instead
/// of `None`. That is deliberately distinct from the sync layer's opaque-leaf
/// marker (`"Opaque"`): a `"JsRoot"` leaf is NOT opaque, so the sync apply path
/// defers it to the WASM `__calimero_merge_root_state` callback (the same path a
/// Rust `#[app::state]` root uses) rather than LWW-collapsing it. Reusing the
/// existing `LwwRegister` variant keeps the pinned borsh discriminants unchanged.
pub const JS_ROOT_CRDT_TYPE_NAME: &str = "JsRoot";

impl CrdtType {
    /// Create an LwwRegister with a known inner type.
    #[must_use]
    pub fn lww_register(inner_type: impl Into<String>) -> Self {
        Self::LwwRegister {
            inner_type: inner_type.into(),
        }
    }

    /// The JS-SDK root marker (see [`JS_ROOT_CRDT_TYPE_NAME`]).
    #[must_use]
    pub fn js_root() -> Self {
        Self::lww_register(JS_ROOT_CRDT_TYPE_NAME)
    }

    /// Whether this is the JS-SDK root marker (see [`JS_ROOT_CRDT_TYPE_NAME`]).
    #[must_use]
    pub fn is_js_root(&self) -> bool {
        matches!(self, Self::LwwRegister { inner_type } if inner_type == JS_ROOT_CRDT_TYPE_NAME)
    }

    /// Create an UnorderedMap with known key and value types.
    #[must_use]
    pub fn unordered_map(key_type: impl Into<String>, value_type: impl Into<String>) -> Self {
        Self::UnorderedMap {
            key_type: key_type.into(),
            value_type: value_type.into(),
        }
    }

    /// Create a SortedMap with known key and value types.
    #[must_use]
    pub fn sorted_map(key_type: impl Into<String>, value_type: impl Into<String>) -> Self {
        Self::SortedMap {
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

    /// Create a SortedSet with a known element type.
    #[must_use]
    pub fn sorted_set(element_type: impl Into<String>) -> Self {
        Self::SortedSet {
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
        matches!(self, Self::UnorderedSet { .. } | Self::SortedSet { .. })
    }

    /// Returns `true` if this is a collection type (map, set, vector, or array).
    #[must_use]
    pub const fn is_collection(&self) -> bool {
        matches!(
            self,
            Self::UnorderedMap { .. }
                | Self::SortedMap { .. }
                | Self::UnorderedSet { .. }
                | Self::SortedSet { .. }
                | Self::Vector { .. }
                | Self::Rga
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
        matches!(
            self,
            Self::UserStorage | Self::FrozenStorage | Self::SharedStorage
        )
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
    fn test_js_root_marker() {
        let js_root = CrdtType::js_root();
        assert_eq!(
            js_root,
            CrdtType::LwwRegister {
                inner_type: JS_ROOT_CRDT_TYPE_NAME.to_string()
            }
        );
        assert!(js_root.is_js_root());
        // A JsRoot root is NOT the opaque-leaf marker the sync layer uses, so it
        // is routed to the WASM merge callback rather than opaque LWW; and an
        // ordinary register is not a JsRoot.
        assert!(!CrdtType::lww_register("Opaque").is_js_root());
        assert!(!CrdtType::lww_register("String").is_js_root());
        assert!(!CrdtType::GCounter.is_js_root());
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
        assert!(CrdtType::sorted_set("String").is_set());
        assert!(!CrdtType::unordered_map("String", "u64").is_set());
        assert!(!CrdtType::sorted_map("String", "u64").is_set());
        assert!(!CrdtType::vector("u64").is_set());
    }

    #[test]
    fn test_is_collection() {
        assert!(CrdtType::unordered_map("String", "u64").is_collection());
        assert!(CrdtType::sorted_map("String", "u64").is_collection());
        assert!(CrdtType::unordered_set("String").is_collection());
        assert!(CrdtType::sorted_set("String").is_collection());
        assert!(CrdtType::vector("u64").is_collection());
        assert!(CrdtType::Rga.is_collection());
        assert!(!CrdtType::lww_register("u64").is_collection());
        assert!(!CrdtType::GCounter.is_collection());
        assert!(!CrdtType::PnCounter.is_collection());
    }

    #[test]
    fn test_sorted_map_constructor() {
        assert_eq!(
            CrdtType::sorted_map("String", "u64"),
            CrdtType::SortedMap {
                key_type: "String".to_string(),
                value_type: "u64".to_string(),
            }
        );
        // SortedMap is distinct from UnorderedMap.
        assert_ne!(
            CrdtType::sorted_map("String", "u64"),
            CrdtType::unordered_map("String", "u64")
        );
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
        assert!(CrdtType::SharedStorage.is_special_storage());
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
            CrdtType::sorted_map("String", "u64"),
            CrdtType::unordered_set("String"),
            CrdtType::sorted_set("String"),
            CrdtType::vector("u64"),
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::SharedStorage,
            CrdtType::Custom("my_type".to_string()),
            CrdtType::RotationLog,
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
            CrdtType::sorted_map("String", "u64"),
            CrdtType::unordered_set("String"),
            CrdtType::sorted_set("String"),
            CrdtType::vector("u64"),
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::SharedStorage,
            CrdtType::Custom("my_type".to_string()),
            CrdtType::RotationLog,
        ];

        for crdt_type in &types {
            let bytes = borsh::to_vec(crdt_type).unwrap();
            let decoded: CrdtType = borsh::from_slice(&bytes).unwrap();
            assert_eq!(*crdt_type, decoded);
        }
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn test_borsh_discriminant_tags_are_stable() {
        // #402: pin borsh tags — reordering variants breaks the wire format.
        let tag = |t: &CrdtType| borsh::to_vec(t).unwrap()[0];

        assert_eq!(tag(&CrdtType::lww_register("x")), 0);
        assert_eq!(tag(&CrdtType::GCounter), 1);
        assert_eq!(tag(&CrdtType::PnCounter), 2);
        assert_eq!(tag(&CrdtType::Rga), 3);
        assert_eq!(tag(&CrdtType::unordered_map("k", "v")), 4);
        assert_eq!(tag(&CrdtType::sorted_map("k", "v")), 5);
        assert_eq!(tag(&CrdtType::unordered_set("e")), 6);
        assert_eq!(tag(&CrdtType::sorted_set("e")), 7);
        assert_eq!(tag(&CrdtType::vector("e")), 8);
        assert_eq!(tag(&CrdtType::UserStorage), 9);
        assert_eq!(tag(&CrdtType::FrozenStorage), 10);
        assert_eq!(tag(&CrdtType::SharedStorage), 11);
        assert_eq!(tag(&CrdtType::Custom("c".into())), 12);
        assert_eq!(tag(&CrdtType::RotationLog), 13);
    }

    // ------------------------------------------------------- frozen encodings
    //
    // Byte-for-byte pins. merobox cannot reach this class of break at all:
    // every node in an e2e run is the SAME build, so a payload or layout change
    // is invisible there and only surfaces against rows already on disk or a
    // peer that has not been restarted yet.

    /// Every variant's complete encoding, as production writes it today.
    ///
    /// The string payloads are frozen EMPTY on purpose: they land in persisted
    /// `EntityIndex` metadata, in `LeafMetadata` on the wire, and in the
    /// `CausalDelta::compute_id` preimage, so filling them from
    /// `std::any::type_name` - whose rendering rustc does not guarantee - makes
    /// all of those bytes compiler-dependent.
    #[cfg(feature = "borsh")]
    #[test]
    fn borsh_encodings_are_byte_frozen() {
        let frozen: &[(CrdtType, &[u8])] = &[
            (CrdtType::lww_register(""), &[0, 0, 0, 0, 0]),
            (CrdtType::GCounter, &[1]),
            (CrdtType::PnCounter, &[2]),
            (CrdtType::Rga, &[3]),
            (
                CrdtType::unordered_map("", ""),
                &[4, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            (CrdtType::sorted_map("", ""), &[5, 0, 0, 0, 0, 0, 0, 0, 0]),
            (CrdtType::unordered_set(""), &[6, 0, 0, 0, 0]),
            (CrdtType::sorted_set(""), &[7, 0, 0, 0, 0]),
            (CrdtType::vector(""), &[8, 0, 0, 0, 0]),
            (CrdtType::UserStorage, &[9]),
            (CrdtType::FrozenStorage, &[10]),
            (CrdtType::SharedStorage, &[11]),
            (
                CrdtType::Custom("my_type".to_owned()),
                &[12, 7, 0, 0, 0, b'm', b'y', b'_', b't', b'y', b'p', b'e'],
            ),
            (CrdtType::RotationLog, &[13]),
            // The one payload production still writes.
            (
                CrdtType::js_root(),
                &[0, 6, 0, 0, 0, b'J', b's', b'R', b'o', b'o', b't'],
            ),
        ];

        for (crdt_type, expected) in frozen {
            assert_eq!(
                borsh::to_vec(crdt_type).unwrap(),
                *expected,
                "encoding drifted for {crdt_type:?}"
            );
        }
    }

    /// Rows and wire frames written before the payload was frozen empty carry
    /// rustc's `type_name` output verbatim. They must still decode - that is
    /// why the fields are retained rather than removed.
    #[cfg(feature = "borsh")]
    #[test]
    fn legacy_type_name_payloads_still_decode() {
        let mut lww = vec![0u8, 21, 0, 0, 0];
        lww.extend_from_slice(b"alloc::string::String");
        let decoded: CrdtType = borsh::from_slice(&lww).unwrap();
        assert_eq!(decoded, CrdtType::lww_register("alloc::string::String"));

        let mut map = vec![4u8, 21, 0, 0, 0];
        map.extend_from_slice(b"alloc::string::String");
        map.extend_from_slice(&[3, 0, 0, 0]);
        map.extend_from_slice(b"u64");
        let decoded: CrdtType = borsh::from_slice(&map).unwrap();
        assert_eq!(
            decoded,
            CrdtType::unordered_map("alloc::string::String", "u64")
        );
    }
}
