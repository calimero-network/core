//! CRDT (Conflict-free Replicated Data Type) primitives.
//!
//! This module provides the unified `CrdtType` enum used across the codebase
//! for identifying CRDT semantics during storage and synchronization.

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// What a [`CrdtType::LwwRegister`] leaf represents.
///
/// Replaces a free-form type-name string whose only real domain was these three
/// cases; `Opaque` and `JsRoot` used to be spelled as magic strings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub enum LwwKind {
    /// An ordinary value leaf.
    #[default]
    Value,
    /// The sync layer's opaque-leaf marker: LWW-merged, never descended into.
    Opaque,
    /// A JS SDK app root carrying a guest `__calimero_merge_root_state` callback.
    JsRoot,
}

impl LwwKind {
    /// Anything that is not a sentinel was a `std::any::type_name` rendering,
    /// i.e. an ordinary value.
    fn from_legacy(inner_type: &str) -> Self {
        match inner_type {
            OPAQUE_LEAF_CRDT_TYPE_NAME => Self::Opaque,
            JS_ROOT_CRDT_TYPE_NAME => Self::JsRoot,
            _ => Self::Value,
        }
    }
}

/// Stable identifier for an app-defined CRDT type.
///
/// A [`CrdtType::Custom`] is stamped on every entry of a custom-valued
/// collection, and entity metadata travels in `LeafMetadata`, lands in
/// persisted rows, and enters `CausalDelta::compute_id`'s preimage through
/// `ancestors`. A type *name* there would put a per-entry string in the hash
/// preimage and on the wire — the surface core#3743 removed from the other
/// variants. This carries a digest of the name instead, so the wire cost is
/// eight bytes regardless of how the app spells its types.
///
/// The digest is over the type's **declared path**, taken from the source
/// token at macro-expansion time. It is deliberately NOT
/// `std::any::type_name`, whose rendering rustc is free to change (it did in
/// 1.98) — the whole reason those strings were removed.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct CustomTypeId(u64);

impl CustomTypeId {
    /// Derive the id for a declared type path, e.g. `"team_metrics::TeamStats"`.
    ///
    /// FNV-1a/64: `const`, dependency-free, and fixed by this source — a hash
    /// whose definition can drift is the same trap as `type_name`, so it is
    /// spelled out here rather than delegated to a crate that may retune.
    #[must_use]
    pub const fn of(path: &str) -> Self {
        let bytes = path.as_bytes();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            i += 1;
        }
        Self(hash)
    }

    /// The raw digest, for the guest-side dispatch table.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

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
pub enum CrdtType {
    // =========================================================================
    // REGISTERS
    // =========================================================================
    /// Last-Writer-Wins Register.
    ///
    /// Wraps primitive types with timestamp-based conflict resolution.
    /// Merge: Higher HLC timestamp wins, with node ID as tie-breaker.
    LwwRegister {
        /// What the leaf represents to the sync layer.
        kind: LwwKind,
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
    UnorderedMap,

    /// Sorted Map.
    ///
    /// Key-value store with the same add-wins merge semantics as
    /// [`UnorderedMap`](Self::UnorderedMap), but iterated in ascending key
    /// order to support range and prefix queries. The ordering is derived from
    /// `K: Ord` and is therefore a pure function of the key set — no extra
    /// state is synced and merge is byte-identical to `UnorderedMap`.
    /// Merge: Union of keys, recursive merge of values.
    SortedMap,

    /// Unordered Set.
    ///
    /// Collection of unique values with add-wins semantics.
    /// Elements are never lost once added.
    /// Merge: Union of all elements from both sets.
    UnorderedSet,

    /// Sorted Set.
    ///
    /// Same add-wins union merge as [`UnorderedSet`](Self::UnorderedSet), but
    /// iterated in ascending element order to support range and prefix queries.
    /// Ordering is derived from `T: Ord` — no extra state is synced.
    /// Merge: Union of all elements from both sets.
    SortedSet,

    /// Vector (ordered collection).
    ///
    /// Ordered list with append operations.
    /// Elements are identified by index + timestamp for ordering.
    /// Merge: Element-wise merge by index with timestamp ordering.
    Vector,

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
    /// Carries a [`CustomTypeId`] the guest resolves to its own merge function.
    /// Merge: dispatched to the WASM runtime to call the app's merge function.
    Custom(CustomTypeId),

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

/// Current-format tags are the legacy tag plus this offset.
///
/// Removing the payload strings is a wire break, so all nodes are expected to
/// upgrade together; a straggler rejects these frames rather than misreading them.
///
/// The payload strings were removed from five variants, so a binary that
/// predates the removal must not read these bytes: an unknown discriminant
/// fails the decode outright, where a shared tag would have it consume the
/// following field as a string length and misalign everything after it.
///
/// `Custom` later traded its name for a [`CustomTypeId`] digest and moved to
/// offset 14 for the same reason, leaving offset 12 as a read-only path for
/// the name spelling.
#[cfg(feature = "borsh")]
const CRDT_TYPE_TAG_V2: u8 = 0x80;

/// Discriminants a pre-removal binary knows, kept for the collision assertion.
#[cfg(all(test, feature = "borsh"))]
const LEGACY_TAGS: [u8; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

#[cfg(feature = "borsh")]
impl BorshSerialize for CrdtType {
    fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
        let tag = |offset: u8| CRDT_TYPE_TAG_V2 + offset;

        match self {
            Self::LwwRegister { kind } => {
                writer.write_all(&[tag(0)])?;
                BorshSerialize::serialize(kind, writer)
            }
            Self::GCounter => writer.write_all(&[tag(1)]),
            Self::PnCounter => writer.write_all(&[tag(2)]),
            Self::Rga => writer.write_all(&[tag(3)]),
            Self::UnorderedMap => writer.write_all(&[tag(4)]),
            Self::SortedMap => writer.write_all(&[tag(5)]),
            Self::UnorderedSet => writer.write_all(&[tag(6)]),
            Self::SortedSet => writer.write_all(&[tag(7)]),
            Self::Vector => writer.write_all(&[tag(8)]),
            Self::UserStorage => writer.write_all(&[tag(9)]),
            Self::FrozenStorage => writer.write_all(&[tag(10)]),
            Self::SharedStorage => writer.write_all(&[tag(11)]),
            Self::RotationLog => writer.write_all(&[tag(13)]),
            // Appended at 14 rather than reusing 12: the payload changed from a
            // name to a digest, and a reader that still expects a string would
            // take the digest's first four bytes as a length and misalign
            // everything after it. An unknown discriminant fails the decode
            // instead — the same reason the whole tag space moved to 0x80.
            Self::Custom(id) => {
                writer.write_all(&[tag(14)])?;
                BorshSerialize::serialize(id, writer)
            }
        }
    }
}

#[cfg(feature = "borsh")]
impl BorshDeserialize for CrdtType {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        let tag = u8::deserialize_reader(reader)?;
        let (variant, legacy) = match tag.checked_sub(CRDT_TYPE_TAG_V2) {
            Some(variant) => (variant, false),
            None => (tag, true),
        };

        match variant {
            0 => {
                let kind = if legacy {
                    LwwKind::from_legacy(&String::deserialize_reader(reader)?)
                } else {
                    LwwKind::deserialize_reader(reader)?
                };
                Ok(Self::LwwRegister { kind })
            }
            1 => Ok(Self::GCounter),
            2 => Ok(Self::PnCounter),
            3 => Ok(Self::Rga),
            4..=8 => {
                if legacy {
                    let payloads = if variant <= 5 { 2 } else { 1 };
                    for _ in 0..payloads {
                        drop(String::deserialize_reader(reader)?);
                    }
                }
                Ok(match variant {
                    4 => Self::UnorderedMap,
                    5 => Self::SortedMap,
                    6 => Self::UnorderedSet,
                    7 => Self::SortedSet,
                    // SAFETY: the arm guard restricts `variant` to 4..=8.
                    _ => Self::Vector,
                })
            }
            9 => Ok(Self::UserStorage),
            10 => Ok(Self::FrozenStorage),
            11 => Ok(Self::SharedStorage),
            // Name-payload `Custom`, from either pre-0x80 rows or the window
            // between the tag move and the digest change. Nothing in production
            // ever constructed one, so this is belt-and-braces, not migration.
            12 => Ok(Self::Custom(CustomTypeId::of(&String::deserialize_reader(
                reader,
            )?))),
            13 => Ok(Self::RotationLog),
            14 if !legacy => Ok(Self::Custom(CustomTypeId::deserialize_reader(reader)?)),
            _ => Err(borsh::io::Error::new(
                borsh::io::ErrorKind::InvalidData,
                "unknown CrdtType discriminant",
            )),
        }
    }
}

impl Default for CrdtType {
    fn default() -> Self {
        Self::LwwRegister {
            kind: LwwKind::Value,
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
/// the runtime stamps the root with `CrdtType::js_root()` instead
/// of `None`. That is deliberately distinct from the sync layer's opaque-leaf
/// marker (`"Opaque"`): a `"JsRoot"` leaf is NOT opaque, so the sync apply path
/// defers it to the WASM `__calimero_merge_root_state` callback (the same path a
/// Rust `#[app::state]` root uses) rather than LWW-collapsing it. Reusing the
/// existing `LwwRegister` variant keeps the pinned borsh discriminants unchanged.
pub const JS_ROOT_CRDT_TYPE_NAME: &str = "JsRoot";

/// Legacy spelling of [`LwwKind::Opaque`], still read from pre-existing rows.
pub const OPAQUE_LEAF_CRDT_TYPE_NAME: &str = "Opaque";

impl CrdtType {
    /// An ordinary value leaf.
    #[must_use]
    pub const fn lww_register() -> Self {
        Self::LwwRegister {
            kind: LwwKind::Value,
        }
    }

    /// The sync layer's opaque-leaf marker.
    #[must_use]
    pub const fn opaque_leaf() -> Self {
        Self::LwwRegister {
            kind: LwwKind::Opaque,
        }
    }

    /// The JS-SDK root marker (see [`LwwKind::JsRoot`]).
    #[must_use]
    pub const fn js_root() -> Self {
        Self::LwwRegister {
            kind: LwwKind::JsRoot,
        }
    }

    /// Whether this is the JS-SDK root marker (see [`LwwKind::JsRoot`]).
    #[must_use]
    pub const fn is_js_root(&self) -> bool {
        matches!(
            self,
            Self::LwwRegister {
                kind: LwwKind::JsRoot
            }
        )
    }

    /// Whether this is the sync layer's opaque leaf (see [`LwwKind::Opaque`]).
    #[must_use]
    pub const fn is_opaque_leaf(&self) -> bool {
        matches!(
            self,
            Self::LwwRegister {
                kind: LwwKind::Opaque
            }
        )
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
        let lww = CrdtType::lww_register();
        assert_eq!(
            lww,
            CrdtType::LwwRegister {
                kind: LwwKind::Value
            }
        );
    }

    #[test]
    fn test_js_root_marker() {
        let js_root = CrdtType::js_root();
        assert_eq!(
            js_root,
            CrdtType::LwwRegister {
                kind: LwwKind::JsRoot
            }
        );
        assert!(js_root.is_js_root());
        // A JsRoot root is NOT the opaque-leaf marker the sync layer uses, so it
        // is routed to the WASM merge callback rather than opaque LWW; and an
        // ordinary register is not a JsRoot.
        assert!(!CrdtType::opaque_leaf().is_js_root());
        assert!(!CrdtType::lww_register().is_js_root());
        assert!(!CrdtType::GCounter.is_js_root());
    }

    #[test]
    fn test_is_counter() {
        assert!(CrdtType::GCounter.is_counter());
        assert!(CrdtType::PnCounter.is_counter());
        assert!(!CrdtType::lww_register().is_counter());
        assert!(!CrdtType::UnorderedMap.is_counter());
    }

    #[test]
    fn test_is_set() {
        assert!(CrdtType::UnorderedSet.is_set());
        assert!(CrdtType::SortedSet.is_set());
        assert!(!CrdtType::UnorderedMap.is_set());
        assert!(!CrdtType::SortedMap.is_set());
        assert!(!CrdtType::Vector.is_set());
    }

    #[test]
    fn test_is_collection() {
        assert!(CrdtType::UnorderedMap.is_collection());
        assert!(CrdtType::SortedMap.is_collection());
        assert!(CrdtType::UnorderedSet.is_collection());
        assert!(CrdtType::SortedSet.is_collection());
        assert!(CrdtType::Vector.is_collection());
        assert!(CrdtType::Rga.is_collection());
        assert!(!CrdtType::lww_register().is_collection());
        assert!(!CrdtType::GCounter.is_collection());
        assert!(!CrdtType::PnCounter.is_collection());
    }

    #[test]
    fn test_sorted_map_constructor() {
        // SortedMap is distinct from UnorderedMap.
        assert_ne!(CrdtType::SortedMap, CrdtType::UnorderedMap);
    }

    #[test]
    fn test_is_custom() {
        assert!(CrdtType::Custom(CustomTypeId::of("test")).is_custom());
        assert!(!CrdtType::lww_register().is_custom());
    }

    #[test]
    fn test_is_special_storage() {
        assert!(CrdtType::UserStorage.is_special_storage());
        assert!(CrdtType::FrozenStorage.is_special_storage());
        assert!(CrdtType::SharedStorage.is_special_storage());
        assert!(!CrdtType::lww_register().is_special_storage());
        assert!(!CrdtType::GCounter.is_special_storage());
    }

    #[test]
    fn test_serde_roundtrip() {
        let types = [
            CrdtType::lww_register(),
            CrdtType::lww_register(),
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::SortedMap,
            CrdtType::UnorderedSet,
            CrdtType::SortedSet,
            CrdtType::Vector,
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::SharedStorage,
            CrdtType::Custom(CustomTypeId::of("my_type")),
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
            CrdtType::lww_register(),
            CrdtType::lww_register(),
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::SortedMap,
            CrdtType::UnorderedSet,
            CrdtType::SortedSet,
            CrdtType::Vector,
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::SharedStorage,
            CrdtType::Custom(CustomTypeId::of("my_type")),
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
        // Reordering variants breaks the wire format; new variants append.
        let tag = |t: &CrdtType| borsh::to_vec(t).unwrap()[0];

        assert_eq!(tag(&CrdtType::lww_register()), CRDT_TYPE_TAG_V2);
        assert_eq!(tag(&CrdtType::GCounter), CRDT_TYPE_TAG_V2 + 1);
        assert_eq!(tag(&CrdtType::PnCounter), CRDT_TYPE_TAG_V2 + 2);
        assert_eq!(tag(&CrdtType::Rga), CRDT_TYPE_TAG_V2 + 3);
        assert_eq!(tag(&CrdtType::UnorderedMap), CRDT_TYPE_TAG_V2 + 4);
        assert_eq!(tag(&CrdtType::SortedMap), CRDT_TYPE_TAG_V2 + 5);
        assert_eq!(tag(&CrdtType::UnorderedSet), CRDT_TYPE_TAG_V2 + 6);
        assert_eq!(tag(&CrdtType::SortedSet), CRDT_TYPE_TAG_V2 + 7);
        assert_eq!(tag(&CrdtType::Vector), CRDT_TYPE_TAG_V2 + 8);
        assert_eq!(tag(&CrdtType::UserStorage), CRDT_TYPE_TAG_V2 + 9);
        assert_eq!(tag(&CrdtType::FrozenStorage), CRDT_TYPE_TAG_V2 + 10);
        assert_eq!(tag(&CrdtType::SharedStorage), CRDT_TYPE_TAG_V2 + 11);
        assert_eq!(tag(&CrdtType::RotationLog), CRDT_TYPE_TAG_V2 + 13);
        // 12 is retired: it was `Custom(String)`, still readable, never written.
        assert_eq!(
            tag(&CrdtType::Custom(CustomTypeId::of("c"))),
            CRDT_TYPE_TAG_V2 + 14
        );
    }

    /// These bytes are persisted, sent on the wire and hashed into `delta_id`,
    /// so the payloads stay empty rather than compiler-dependent.
    #[cfg(feature = "borsh")]
    #[test]
    fn borsh_encodings_are_byte_frozen() {
        let frozen: &[(CrdtType, &[u8])] = &[
            (CrdtType::lww_register(), &[0x80, 0]),
            (CrdtType::opaque_leaf(), &[0x80, 1]),
            (CrdtType::js_root(), &[0x80, 2]),
            (CrdtType::GCounter, &[0x81]),
            (CrdtType::PnCounter, &[0x82]),
            (CrdtType::Rga, &[0x83]),
            (CrdtType::UnorderedMap, &[0x84]),
            (CrdtType::SortedMap, &[0x85]),
            (CrdtType::UnorderedSet, &[0x86]),
            (CrdtType::SortedSet, &[0x87]),
            (CrdtType::Vector, &[0x88]),
            (CrdtType::UserStorage, &[0x89]),
            (CrdtType::FrozenStorage, &[0x8A]),
            (CrdtType::SharedStorage, &[0x8B]),
            (CrdtType::RotationLog, &[0x8D]),
            (
                CrdtType::Custom(CustomTypeId::of("my_type")),
                &[0x8E, 172, 230, 240, 133, 239, 162, 33, 227],
            ),
        ];

        for (crdt_type, expected) in frozen {
            assert_eq!(
                borsh::to_vec(crdt_type).unwrap(),
                *expected,
                "encoding drifted for {crdt_type:?}"
            );
            assert_eq!(
                &borsh::from_slice::<CrdtType>(expected).unwrap(),
                crdt_type,
                "round-trip drifted for {crdt_type:?}"
            );
        }
    }

    /// Rows written before the payload was frozen carry `type_name` output, and
    /// must still decode - which is why the fields are kept rather than removed.
    #[cfg(feature = "borsh")]
    #[test]
    fn legacy_type_name_payloads_still_decode() {
        let legacy = |tag: u8, payloads: &[&str]| {
            let mut bytes = vec![tag];
            for payload in payloads {
                bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
                bytes.extend_from_slice(payload.as_bytes());
            }
            borsh::from_slice::<CrdtType>(&bytes).unwrap()
        };

        // A type-name payload is an ordinary value; the two sentinels are not.
        assert_eq!(
            legacy(0, &["alloc::string::String"]),
            CrdtType::lww_register()
        );
        assert_eq!(
            legacy(0, &[OPAQUE_LEAF_CRDT_TYPE_NAME]),
            CrdtType::opaque_leaf()
        );
        assert_eq!(legacy(0, &[JS_ROOT_CRDT_TYPE_NAME]), CrdtType::js_root());

        assert_eq!(legacy(4, &["String", "u64"]), CrdtType::UnorderedMap);
        assert_eq!(legacy(5, &["String", "u64"]), CrdtType::SortedMap);
        assert_eq!(legacy(6, &["String"]), CrdtType::UnorderedSet);
        assert_eq!(legacy(7, &["String"]), CrdtType::SortedSet);
        assert_eq!(legacy(8, &["u64"]), CrdtType::Vector);

        // Payload-free legacy variants are unchanged apart from their tag.
        assert_eq!(legacy(1, &[]), CrdtType::GCounter);
        assert_eq!(legacy(13, &[]), CrdtType::RotationLog);

        // A name-payload `Custom` resolves to the digest of that name, from
        // either tag space.
        assert_eq!(
            legacy(12, &["my_type"]),
            CrdtType::Custom(CustomTypeId::of("my_type"))
        );
        assert_eq!(
            legacy(CRDT_TYPE_TAG_V2 + 12, &["my_type"]),
            CrdtType::Custom(CustomTypeId::of("my_type"))
        );
    }

    /// `CustomTypeId` is stamped on entries, travels in `LeafMetadata` and
    /// enters `compute_id`'s preimage, so the digest function is wire format.
    /// Retuning it silently re-labels every custom entry in the network.
    #[test]
    fn custom_type_id_digest_is_frozen() {
        assert_eq!(CustomTypeId::of("my_type").get(), 0xe321_a2ef_85f0_e6ac);
        assert_eq!(
            CustomTypeId::of("team_metrics::TeamStats").get(),
            0x6e4a_e421_df5b_87da
        );
        // FNV-1a's offset basis, i.e. the empty path hashes to a fixed value
        // rather than to zero - `Default` must stay distinguishable from it.
        assert_eq!(CustomTypeId::of("").get(), 0xcbf2_9ce4_8422_2325);
        assert_ne!(CustomTypeId::of(""), CustomTypeId::default());
    }

    /// A binary predating the payload removal must reject current bytes rather
    /// than read the following field as a string length and shift everything.
    #[cfg(feature = "borsh")]
    #[test]
    fn current_tags_are_unknown_to_a_legacy_reader() {
        for tag in 0x80..=0x8E_u8 {
            assert!(
                LEGACY_TAGS.binary_search(&tag).is_err(),
                "tag {tag:#x} collides with a legacy discriminant"
            );
        }

        // The mirror of that: this build rejects a tag from neither range.
        assert!(borsh::from_slice::<CrdtType>(&[0x7F]).is_err());
        assert!(borsh::from_slice::<CrdtType>(&[0xFF]).is_err());
    }
}
