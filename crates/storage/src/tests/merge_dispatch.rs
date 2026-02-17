//! CRDT Type-Based Merge Dispatch Tests
//!
//! These tests verify that the storage layer correctly dispatches merge operations
//! based on `CrdtType` metadata, rather than using LWW for all non-root entities.
//!
//! **Invariant I5**: Initialized nodes MUST CRDT-merge; overwrite ONLY for fresh nodes.
//!
//! ## Test Coverage
//!
//! | Test | CRDT Type | Expected Behavior |
//! |------|-----------|-------------------|
//! | `test_gcounter_merge_sums_contributions` | GCounter | Max per contributor |
//! | `test_pncounter_merge_combines_maps` | PnCounter | Union of pos/neg maps |
//! | `test_lww_register_returns_wasm_required` | LwwRegister | Needs WASM callback |
//! | `test_is_builtin_crdt_classification` | Various | Correct classification |
//! | `test_merge_by_crdt_type_dispatch` | Various | Correct dispatch |

use calimero_primitives::crdt::CrdtType;
use serial_test::serial;

use crate::collections::crdt_meta::MergeError;
use crate::collections::Counter;
use crate::env;
use crate::merge::{is_builtin_crdt, merge_by_crdt_type};

// =============================================================================
// Test Helpers
// =============================================================================

/// Create a GCounter with a specific executor's contribution.
fn create_gcounter_with_contribution(executor_id: [u8; 32], count: u64) -> Counter<false> {
    env::set_executor_id(executor_id);
    let mut counter = Counter::new();
    for _ in 0..count {
        counter.increment().unwrap();
    }
    counter
}

/// Create a PnCounter with specific positive and negative contributions.
fn create_pncounter_with_contributions(
    executor_id: [u8; 32],
    positive: u64,
    negative: u64,
) -> Counter<true> {
    env::set_executor_id(executor_id);
    let mut counter = Counter::new();
    for _ in 0..positive {
        counter.increment().unwrap();
    }
    for _ in 0..negative {
        counter.decrement().unwrap();
    }
    counter
}

// =============================================================================
// Tests for merge_by_crdt_type
// =============================================================================

/// Test: GCounter merge should combine contributions from different executors.
///
/// When two nodes increment a GCounter with different executor IDs,
/// the merge should preserve both contributions (take max per executor).
#[test]
#[serial]
fn test_gcounter_merge_sums_contributions() {
    env::reset_for_testing();

    // Node A: executor [0xAA; 32] contributed 5
    let alice_executor = [0xAA; 32];
    let alice_counter = create_gcounter_with_contribution(alice_executor, 5);
    let alice_bytes = borsh::to_vec(&alice_counter).unwrap();

    // Node B: executor [0xBB; 32] contributed 3
    let bob_executor = [0xBB; 32];
    let bob_counter = create_gcounter_with_contribution(bob_executor, 3);
    let bob_bytes = borsh::to_vec(&bob_counter).unwrap();

    // Merge using merge_by_crdt_type
    let merged_bytes = merge_by_crdt_type(&CrdtType::GCounter, &alice_bytes, &bob_bytes).unwrap();

    // Deserialize and verify
    let merged: Counter<false> = borsh::from_slice(&merged_bytes).unwrap();
    let merged_value = merged.value().unwrap();

    // Expected: alice(5) + bob(3) = 8
    assert_eq!(
        merged_value, 8,
        "GCounter merge should sum contributions from different executors: 5 + 3 = 8, got {}",
        merged_value
    );
}

/// Test: GCounter merge with same executor takes MAX.
#[test]
#[serial]
fn test_gcounter_merge_max_per_executor() {
    env::reset_for_testing();

    let executor = [0xAA; 32];

    // Node A: executor contributed 5
    let counter_a = create_gcounter_with_contribution(executor, 5);
    let bytes_a = borsh::to_vec(&counter_a).unwrap();

    // Node B: same executor contributed 7 (saw more operations)
    let counter_b = create_gcounter_with_contribution(executor, 7);
    let bytes_b = borsh::to_vec(&counter_b).unwrap();

    // Merge
    let merged_bytes = merge_by_crdt_type(&CrdtType::GCounter, &bytes_a, &bytes_b).unwrap();

    let merged: Counter<false> = borsh::from_slice(&merged_bytes).unwrap();
    let merged_value = merged.value().unwrap();

    // Expected: max(5, 7) = 7
    assert_eq!(
        merged_value, 7,
        "GCounter merge should take max for same executor: max(5, 7) = 7, got {}",
        merged_value
    );
}

/// Test: PnCounter merge combines positive and negative maps.
#[test]
#[serial]
fn test_pncounter_merge_combines_maps() {
    env::reset_for_testing();

    let alice = [0xAA; 32];
    let bob = [0xBB; 32];

    // Node A: alice +10, -2 → value = 8
    let counter_a = create_pncounter_with_contributions(alice, 10, 2);
    let bytes_a = borsh::to_vec(&counter_a).unwrap();

    // Node B: bob +5 → value = 5
    let counter_b = create_pncounter_with_contributions(bob, 5, 0);
    let bytes_b = borsh::to_vec(&counter_b).unwrap();

    // Merge
    let merged_bytes = merge_by_crdt_type(&CrdtType::PnCounter, &bytes_a, &bytes_b).unwrap();

    let merged: Counter<true> = borsh::from_slice(&merged_bytes).unwrap();
    let merged_value = merged.value().unwrap();

    // Expected: positive(alice:10, bob:5) - negative(alice:2) = 15 - 2 = 13
    assert_eq!(
        merged_value, 13,
        "PnCounter merge should combine maps: (10+5) - 2 = 13, got {}",
        merged_value
    );
}

/// Test: LwwRegister returns incoming bytes (LWW semantics).
///
/// LwwRegister merge uses metadata timestamps in the caller (try_merge_non_root).
/// The merge_by_crdt_type function just returns incoming - the actual timestamp
/// comparison happens at the interface level.
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

// =============================================================================
// Tests for is_builtin_crdt
// =============================================================================

/// Test: is_builtin_crdt correctly classifies CRDT types.
#[test]
fn test_is_builtin_crdt_classification() {
    // Built-in types that can be merged at byte level:
    assert!(
        is_builtin_crdt(&CrdtType::GCounter),
        "GCounter should be builtin"
    );
    assert!(
        is_builtin_crdt(&CrdtType::PnCounter),
        "PnCounter should be builtin"
    );
    assert!(is_builtin_crdt(&CrdtType::Rga), "Rga should be builtin");

    // All standard types are builtin
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
        !is_builtin_crdt(&CrdtType::Custom("MyType".to_string())),
        "Custom needs WASM"
    );
}

// =============================================================================
// Tests for MergeError variants
// =============================================================================

/// Test: WasmRequired error for Custom types.
#[test]
fn test_merge_custom_type_returns_wasm_required() {
    let bytes = vec![1, 2, 3, 4];

    let result = merge_by_crdt_type(
        &CrdtType::Custom("MyCustomType".to_string()),
        &bytes,
        &bytes,
    );

    assert!(
        matches!(result, Err(MergeError::WasmRequired { type_name }) if type_name == "MyCustomType"),
        "Custom types should return WasmRequired with the type name"
    );
}

/// Test: SerializationError for corrupted data.
#[test]
#[serial]
fn test_merge_corrupted_data_returns_serialization_error() {
    env::reset_for_testing();

    let corrupted = vec![0xFF, 0xFF, 0xFF, 0xFF]; // Invalid GCounter encoding

    // Create a valid counter for the other side
    let valid_counter = create_gcounter_with_contribution([0xAA; 32], 5);
    let valid = borsh::to_vec(&valid_counter).unwrap();

    let result = merge_by_crdt_type(&CrdtType::GCounter, &corrupted, &valid);

    assert!(
        matches!(result, Err(MergeError::SerializationError(_))),
        "Corrupted data should return SerializationError, got {:?}",
        result
    );
}

/// Test: Collections return incoming (structural merge).
#[test]
fn test_collections_return_incoming() {
    let existing = vec![1, 2, 3, 4];
    let incoming = vec![5, 6, 7, 8];

    // UnorderedMap - returns incoming for structured storage
    let result = merge_by_crdt_type(
        &CrdtType::unordered_map("String", "u64"),
        &existing,
        &incoming,
    );
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), incoming);

    // UnorderedSet - returns incoming for structured storage
    let result = merge_by_crdt_type(&CrdtType::unordered_set("String"), &existing, &incoming);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), incoming);

    // Vector - returns incoming for structured storage
    let result = merge_by_crdt_type(&CrdtType::vector("u64"), &existing, &incoming);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), incoming);
}

// =============================================================================
// Integration test: Verify merge_by_crdt_type is correct
// =============================================================================

/// Verify that merge_by_crdt_type produces different results than LWW for counters.
///
/// This test demonstrates why type-based dispatch matters:
/// - LWW would just pick one value (data loss!)
/// - CRDT merge combines contributions correctly
#[test]
#[serial]
fn test_merge_by_crdt_type_differs_from_lww() {
    env::reset_for_testing();

    // Create two counters from different executors
    let alice_counter = create_gcounter_with_contribution([0xAA; 32], 5);
    let alice_bytes = borsh::to_vec(&alice_counter).unwrap();

    let bob_counter = create_gcounter_with_contribution([0xBB; 32], 3);
    let bob_bytes = borsh::to_vec(&bob_counter).unwrap();

    // LWW would just pick one (say, bob since it's "incoming")
    let lww_result = bob_bytes.clone();
    let lww_counter: Counter<false> = borsh::from_slice(&lww_result).unwrap();
    let lww_value = lww_counter.value().unwrap();

    // CRDT merge combines both
    let merged_bytes = merge_by_crdt_type(&CrdtType::GCounter, &alice_bytes, &bob_bytes).unwrap();
    let merged_counter: Counter<false> = borsh::from_slice(&merged_bytes).unwrap();
    let merged_value = merged_counter.value().unwrap();

    // Verify they're different
    assert_ne!(
        lww_value, merged_value,
        "LWW ({}) should differ from CRDT merge ({})",
        lww_value, merged_value
    );
    assert_eq!(lww_value, 3, "LWW should be bob's value only");
    assert_eq!(merged_value, 8, "CRDT merge should be alice + bob = 8");
}

// =============================================================================
// Compile-time check: Ensure CrdtType has expected variants
// =============================================================================

#[test]
fn test_crdt_type_has_required_variants() {
    // Verify the CrdtType enum has the variants we need for dispatch
    let _ = CrdtType::GCounter;
    let _ = CrdtType::PnCounter;
    let _ = CrdtType::lww_register("test");
    let _ = CrdtType::Rga;
    let _ = CrdtType::unordered_map("String", "u64");
    let _ = CrdtType::unordered_set("String");
    let _ = CrdtType::vector("u64");
    let _ = CrdtType::UserStorage;
    let _ = CrdtType::FrozenStorage;
    let _ = CrdtType::Custom("test".to_string());
}
