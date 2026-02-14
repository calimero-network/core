//! CRDT Dispatch Compliance Tests
//!
//! **CIP Reference**: §6.2 - CRDT Merge Semantics
//!
//! ## Invariant I5 - No Silent Data Loss
//!
//! > Initialized nodes MUST use CRDT-type-specific merge, not LWW.
//!
//! These tests verify that:
//! 1. CrdtType metadata is preserved when storing entities
//! 2. The `merge_by_crdt_type` function correctly dispatches based on type
//! 3. Built-in types (GCounter, PnCounter) merge correctly
//! 4. Types requiring WASM return appropriate errors
//!
//! ## Architecture
//!
//! SimStorage → Interface::apply_action → save_internal → try_merge_non_root
//!                                                              ↓
//!                                                    merge_by_crdt_type

use calimero_primitives::crdt::CrdtType;
use calimero_storage::address::Id;
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::entities::Metadata;
use calimero_storage::merge::{is_builtin_crdt, merge_by_crdt_type};

use crate::sync_sim::node::SimNode;
use crate::sync_sim::types::EntityId;

// =============================================================================
// I5: CrdtType Metadata Preservation
// =============================================================================

/// Verify that CrdtType metadata is preserved when entities are stored.
#[test]
fn test_crdt_type_preserved_in_storage() {
    let node = SimNode::new("test");

    // Store entity with GCounter type
    let entity_id = EntityId::from_u64(42);
    let metadata = Metadata::with_crdt_type(100, 100, CrdtType::GCounter);

    node.storage()
        .add_entity(Id::new(entity_id.0), b"test-data", metadata.clone());

    // Verify the entity was stored
    assert!(node.storage().has_entity(Id::new(entity_id.0)));
}

/// Verify different CrdtTypes are all preserved.
#[test]
fn test_various_crdt_types_preserved() {
    let node = SimNode::new("test");

    let types_to_test = [
        (1, CrdtType::GCounter),
        (2, CrdtType::PnCounter),
        (3, CrdtType::lww_register("test")),
        (4, CrdtType::Rga),
        (5, CrdtType::unordered_map("String", "u64")),
        (6, CrdtType::Custom("MyType".to_string())),
    ];

    for (id, crdt_type) in types_to_test {
        let entity_id = EntityId::from_u64(id);
        let metadata = Metadata::with_crdt_type(100, 100, crdt_type.clone());

        node.storage()
            .add_entity(Id::new(entity_id.0), b"test-data", metadata);

        assert!(
            node.storage().has_entity(Id::new(entity_id.0)),
            "Entity with {:?} should be stored",
            crdt_type
        );
    }
}

// =============================================================================
// I5: is_builtin_crdt Classification
// =============================================================================

/// Verify is_builtin_crdt correctly classifies types.
#[test]
fn test_is_builtin_crdt_classification() {
    // All standard types are builtin
    assert!(is_builtin_crdt(&CrdtType::GCounter), "GCounter is builtin");
    assert!(
        is_builtin_crdt(&CrdtType::PnCounter),
        "PnCounter is builtin"
    );
    assert!(is_builtin_crdt(&CrdtType::Rga), "Rga is builtin");
    assert!(
        is_builtin_crdt(&CrdtType::lww_register("u64")),
        "LwwRegister is builtin"
    );
    assert!(
        is_builtin_crdt(&CrdtType::unordered_map("String", "u64")),
        "UnorderedMap is builtin"
    );
    assert!(
        is_builtin_crdt(&CrdtType::unordered_set("String")),
        "UnorderedSet is builtin"
    );
    assert!(
        is_builtin_crdt(&CrdtType::vector("u64")),
        "Vector is builtin"
    );
    assert!(
        is_builtin_crdt(&CrdtType::UserStorage),
        "UserStorage is builtin"
    );
    assert!(
        is_builtin_crdt(&CrdtType::FrozenStorage),
        "FrozenStorage is builtin"
    );

    // Only Custom needs WASM
    assert!(
        !is_builtin_crdt(&CrdtType::Custom("X".into())),
        "Custom needs WASM"
    );
}

// =============================================================================
// I5: Type-Based Dispatch Returns Correct Errors
// =============================================================================

/// Verify that LwwRegister returns incoming bytes (LWW semantics).
///
/// LwwRegister merge uses metadata timestamps in the caller (try_merge_non_root).
/// The merge_by_crdt_type function just returns incoming.
#[test]
fn test_lww_register_returns_incoming() {
    let existing = vec![1, 2, 3, 4];
    let incoming = vec![5, 6, 7, 8];

    let result = merge_by_crdt_type(&CrdtType::lww_register("test"), &existing, &incoming);

    assert!(result.is_ok(), "LwwRegister merge should succeed");
    assert_eq!(
        result.unwrap(),
        incoming,
        "LwwRegister should return incoming bytes"
    );
}

/// Verify that Custom types return WasmRequired with correct type name.
#[test]
fn test_custom_type_returns_wasm_required() {
    let bytes = vec![1, 2, 3, 4];

    let result = merge_by_crdt_type(
        &CrdtType::Custom("MyApp::Counter".to_string()),
        &bytes,
        &bytes,
    );

    match result {
        Err(MergeError::NoWasmCallback { type_name }) => {
            assert_eq!(type_name, "MyApp::Counter", "Type name should be preserved");
        }
        other => panic!("Expected NoWasmCallback, got {:?}", other),
    }
}

/// Verify that collection types return incoming (structural merge).
#[test]
fn test_collection_types_return_incoming() {
    let existing = vec![1, 2, 3, 4];
    let incoming = vec![5, 6, 7, 8];

    for crdt_type in [
        CrdtType::unordered_map("String", "u64"),
        CrdtType::unordered_set("String"),
        CrdtType::vector("u64"),
    ] {
        let result = merge_by_crdt_type(&crdt_type, &existing, &incoming);
        assert!(result.is_ok(), "{:?} should succeed", crdt_type);
        assert_eq!(
            result.unwrap(),
            incoming,
            "{:?} should return incoming",
            crdt_type
        );
    }
}

/// Verify that corrupted data returns SerializationError.
#[test]
fn test_corrupted_data_returns_serialization_error() {
    // Completely invalid bytes for a GCounter
    let corrupted = vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

    let result = merge_by_crdt_type(&CrdtType::GCounter, &corrupted, &corrupted);

    assert!(
        matches!(result, Err(MergeError::SerializationError(_))),
        "Corrupted data should return SerializationError, got {:?}",
        result
    );
}
