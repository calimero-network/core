//! Simulation tests for builtin CRDT merge behavior.
//!
//! These tests verify that each builtin CRDT type merges correctly
//! according to its documented semantics.
//!
//! ## Test Coverage Matrix
//!
//! | CRDT Type | Merge Semantics | Test |
//! |-----------|-----------------|------|
//! | GCounter | Max per executor | `test_gcounter_merge_*` |
//! | PnCounter | Max per executor (pos/neg) | `test_pncounter_merge_*` |
//! | Rga | Interleave by timestamp | `test_rga_merge_*` |
//! | LwwRegister | Return incoming | `test_lww_register_*` |
//! | UnorderedMap | Return incoming | `test_unordered_map_*` |
//! | UnorderedSet | Return incoming | `test_unordered_set_*` |
//! | Vector | Return incoming | `test_vector_*` |
//! | UserStorage | Return incoming (LWW) | `test_user_storage_*` |
//! | FrozenStorage | Return existing (FWW) | `test_frozen_storage_*` |
//! | Custom | WasmRequired error | `test_custom_*` |

use calimero_primitives::crdt::CrdtType;
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::merge::{is_builtin_crdt, merge_by_crdt_type};

// =============================================================================
// GCounter Tests
// =============================================================================

/// GCounter is classified as builtin (no WASM required).
///
/// Expected merge behavior (tested in merge_dispatch.rs):
/// - Merge combines contributions from different executors
/// - When two nodes increment with different executor IDs,
///   merge takes max per executor, resulting in sum of unique contributions
/// - Same executor takes max (idempotent)
///
/// Actual merge tests in:
/// - crates/storage/src/tests/merge_dispatch.rs::test_gcounter_merge_sums_contributions
/// - crates/storage/src/tests/merge_dispatch.rs::test_gcounter_merge_max_per_executor
#[test]
fn test_gcounter_is_builtin() {
    assert!(
        is_builtin_crdt(&CrdtType::GCounter),
        "GCounter should be builtin"
    );
}

// =============================================================================
// PnCounter Tests
// =============================================================================

/// PnCounter is classified as builtin (no WASM required).
///
/// Expected merge behavior (tested in merge_dispatch.rs):
/// - PnCounter = positive_map - negative_map
/// - Merge: union of positive maps, union of negative maps
/// - positive_map: max per executor
/// - negative_map: max per executor
/// - Can represent negative values if decrements exceed increments
///
/// Actual test in: merge_dispatch.rs::test_pncounter_merge_combines_maps
#[test]
fn test_pncounter_is_builtin() {
    assert!(
        is_builtin_crdt(&CrdtType::PnCounter),
        "PnCounter should be builtin"
    );
}

// =============================================================================
// RGA Tests
// =============================================================================

/// RGA is classified as builtin (no WASM required).
///
/// Expected merge behavior (tested in crdt_impls.rs):
/// - Merge interleaves characters by (timestamp, node_id)
/// - All characters from both RGAs are preserved
/// - Ordering determined by unique timestamps for determinism
/// - Deletions are tracked via tombstones
///
/// Actual tests in:
/// - crdt_impls.rs::test_rga_merge_disjoint_characters
/// - crdt_impls.rs::test_rga_merge_overlapping_edits
/// - crdt_impls.rs::test_rga_merge_with_deletions
#[test]
fn test_rga_is_builtin() {
    assert!(is_builtin_crdt(&CrdtType::Rga), "RGA should be builtin");
}

// =============================================================================
// LwwRegister Tests
// =============================================================================

/// LwwRegister: merge returns incoming (caller uses metadata timestamps).
#[test]
fn test_lww_register_returns_incoming() {
    let existing = vec![1, 2, 3, 4];
    let incoming = vec![5, 6, 7, 8];

    let result = merge_by_crdt_type(&CrdtType::lww_register("test"), &existing, &incoming).unwrap();

    assert_eq!(result, incoming, "LwwRegister should return incoming bytes");
}

/// LwwRegister: different inner types still work (opaque bytes).
#[test]
fn test_lww_register_any_inner_type() {
    // String inner type
    let result = merge_by_crdt_type(
        &CrdtType::lww_register("String"),
        b"old_value",
        b"new_value",
    )
    .unwrap();
    assert_eq!(result, b"new_value");

    // u64 inner type
    let result = merge_by_crdt_type(
        &CrdtType::lww_register("u64"),
        &42u64.to_le_bytes(),
        &99u64.to_le_bytes(),
    )
    .unwrap();
    assert_eq!(result, 99u64.to_le_bytes());
}

// =============================================================================
// Collection Tests (UnorderedMap, UnorderedSet, Vector)
// =============================================================================

/// UnorderedMap: returns incoming (entries sync separately).
#[test]
fn test_unordered_map_returns_incoming() {
    let existing = vec![1, 2, 3];
    let incoming = vec![4, 5, 6];

    let result = merge_by_crdt_type(
        &CrdtType::unordered_map("String", "u64"),
        &existing,
        &incoming,
    )
    .unwrap();

    assert_eq!(result, incoming, "UnorderedMap should return incoming");
}

/// UnorderedSet: returns incoming (elements sync separately).
#[test]
fn test_unordered_set_returns_incoming() {
    let existing = vec![10, 20, 30];
    let incoming = vec![40, 50, 60];

    let result =
        merge_by_crdt_type(&CrdtType::unordered_set("String"), &existing, &incoming).unwrap();

    assert_eq!(result, incoming, "UnorderedSet should return incoming");
}

/// Vector: returns incoming (elements sync separately).
#[test]
fn test_vector_returns_incoming() {
    let existing = vec![100u8, 200];
    let incoming = vec![150u8, 250];

    let result = merge_by_crdt_type(&CrdtType::vector("u64"), &existing, &incoming).unwrap();

    assert_eq!(result, incoming, "Vector should return incoming");
}

// =============================================================================
// UserStorage Tests
// =============================================================================

/// UserStorage: LWW semantics (returns incoming).
#[test]
fn test_user_storage_returns_incoming() {
    let existing = b"user_data_v1".to_vec();
    let incoming = b"user_data_v2".to_vec();

    let result = merge_by_crdt_type(&CrdtType::UserStorage, &existing, &incoming).unwrap();

    assert_eq!(result, incoming, "UserStorage should return incoming (LWW)");
}

// =============================================================================
// FrozenStorage Tests
// =============================================================================
//
// FrozenStorage uses first-write-wins semantics. This is intentionally NOT
// convergent in the CRDT sense: if two nodes independently write different
// values before syncing, they will each keep their own value.
//
// This is by design for immutable data like:
// - Identity keys (once set, never change)
// - Genesis state (established at creation)
// - Application-specific constants
//
// For data that must converge, use LwwRegister or UserStorage instead.

/// FrozenStorage: first-write-wins (keeps existing).
#[test]
fn test_frozen_storage_keeps_existing() {
    let existing = b"immutable_data".to_vec();
    let incoming = b"attempted_overwrite".to_vec();

    let result = merge_by_crdt_type(&CrdtType::FrozenStorage, &existing, &incoming).unwrap();

    assert_eq!(
        result, existing,
        "FrozenStorage should keep existing (first-write-wins)"
    );
}

/// FrozenStorage: empty existing is still a valid first-write.
///
/// An entity that exists with empty bytes is considered "initialized" -
/// the empty value was intentionally written. This is distinct from
/// a non-existent entity (which would take the incoming value).
#[test]
fn test_frozen_storage_keeps_empty_existing() {
    let existing = vec![];
    let incoming = b"new_data".to_vec();

    let result = merge_by_crdt_type(&CrdtType::FrozenStorage, &existing, &incoming).unwrap();

    assert_eq!(
        result, existing,
        "FrozenStorage should keep existing even if empty"
    );
}

// =============================================================================
// Custom Type Tests
// =============================================================================

/// Custom: returns WasmRequired error.
#[test]
fn test_custom_requires_wasm() {
    let bytes = vec![1, 2, 3];

    let result = merge_by_crdt_type(
        &CrdtType::Custom("MyApp::CustomType".into()),
        &bytes,
        &bytes,
    );

    assert!(
        matches!(result, Err(MergeError::WasmRequired { type_name }) if type_name == "MyApp::CustomType"),
        "Custom types should return WasmRequired with type name"
    );
}

// =============================================================================
// Error Handling Tests
// =============================================================================

/// Invalid GCounter bytes return SerializationError.
///
/// Tests that the merge function properly validates Borsh deserialization
/// and returns an error for malformed data.
#[test]
fn test_invalid_gcounter_returns_serialization_error() {
    // Bytes that cannot be deserialized as a GCounter
    let invalid_bytes = vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

    let result = merge_by_crdt_type(&CrdtType::GCounter, &invalid_bytes, &invalid_bytes);

    assert!(
        matches!(result, Err(MergeError::SerializationError(_))),
        "Invalid GCounter bytes should return SerializationError, got {:?}",
        result
    );
}

/// Invalid RGA bytes return SerializationError.
#[test]
fn test_invalid_rga_returns_serialization_error() {
    // Bytes that cannot be deserialized as an RGA
    let invalid_bytes = vec![0xFF, 0xFF, 0xFF];

    let result = merge_by_crdt_type(&CrdtType::Rga, &invalid_bytes, &invalid_bytes);

    assert!(
        matches!(result, Err(MergeError::SerializationError(_))),
        "Invalid RGA bytes should return SerializationError, got {:?}",
        result
    );
}

// =============================================================================
// is_builtin_crdt Classification Tests
// =============================================================================

/// All standard types are builtin (only Custom needs WASM).
#[test]
fn test_all_builtin_types_classification() {
    let builtin_types = [
        CrdtType::GCounter,
        CrdtType::PnCounter,
        CrdtType::Rga,
        CrdtType::lww_register("u64"),
        CrdtType::unordered_map("String", "u64"),
        CrdtType::unordered_set("String"),
        CrdtType::vector("u64"),
        CrdtType::UserStorage,
        CrdtType::FrozenStorage,
    ];

    for crdt_type in &builtin_types {
        assert!(
            is_builtin_crdt(crdt_type),
            "{:?} should be builtin",
            crdt_type
        );
    }

    // Custom is the only non-builtin
    assert!(
        !is_builtin_crdt(&CrdtType::Custom("X".into())),
        "Custom should NOT be builtin"
    );
}

// =============================================================================
// Summary Test
// =============================================================================

/// Summary: Verify merge behavior for all builtin types.
///
/// | Type | Merge Behavior |
/// |------|----------------|
/// | GCounter | Max per executor (deserialization needed) |
/// | PnCounter | Max per executor for +/- (deserialization needed) |
/// | Rga | Interleave by timestamp (deserialization needed) |
/// | LwwRegister | Return incoming |
/// | UnorderedMap | Return incoming (structured storage) |
/// | UnorderedSet | Return incoming (structured storage) |
/// | Vector | Return incoming (structured storage) |
/// | UserStorage | Return incoming (LWW) |
/// | FrozenStorage | Return existing (first-write-wins) |
/// | Custom | WasmRequired error |
#[test]
fn test_builtin_merge_behavior_summary() {
    let existing = b"existing".to_vec();
    let incoming = b"incoming".to_vec();

    // Types that return incoming
    let return_incoming = [
        CrdtType::lww_register("u64"),
        CrdtType::unordered_map("String", "u64"),
        CrdtType::unordered_set("String"),
        CrdtType::vector("u64"),
        CrdtType::UserStorage,
    ];

    for crdt_type in &return_incoming {
        let result = merge_by_crdt_type(crdt_type, &existing, &incoming).unwrap();
        assert_eq!(result, incoming, "{:?} should return incoming", crdt_type);
    }

    // FrozenStorage returns existing
    let result = merge_by_crdt_type(&CrdtType::FrozenStorage, &existing, &incoming).unwrap();
    assert_eq!(result, existing, "FrozenStorage should return existing");

    // Custom returns error
    let result = merge_by_crdt_type(&CrdtType::Custom("X".into()), &existing, &incoming);
    assert!(matches!(result, Err(MergeError::WasmRequired { .. })));
}
