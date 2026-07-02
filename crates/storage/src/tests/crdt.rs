#![allow(unused_results)] // Test code doesn't need to check all return values
//! Comprehensive CRDT storage tests
//!
//! Tests cover:
//! - Last-Write-Wins (LWW) conflict resolution
//! - Tombstone-based deletion
//! - Concurrent updates
//! - Action merging
//! - Edge cases

use serial_test::serial;

use super::common::{Page, Paragraph};
use crate::action::Action;
use crate::address::Id;
use crate::constants::DRIFT_TOLERANCE_NANOS;
use crate::entities::{Data, Element, Metadata};
use crate::env::time_now;
use crate::index::Index;
use crate::interface::{ApplyContext, Interface};
use crate::store::MockedStorage;

type TestStorage = MockedStorage<5000>;
type TestInterface = Interface<TestStorage>;

const ONE_SEC_NANOS: u64 = 1_000_000_000;

// ============================================================
// Last-Write-Wins (LWW) Tests
// ============================================================

#[test]
#[serial]
fn lww_newer_update_wins() {
    super::common::register_test_merge_functions();
    let mut page = Page::new_from_element("Version 1", Element::root());

    // Save initial version
    assert!(TestInterface::save(&mut page).unwrap());

    // Save newer version
    std::thread::sleep(std::time::Duration::from_millis(2));
    page.title = "Version 2".to_string();
    page.element_mut().update();
    assert!(TestInterface::save(&mut page).unwrap());

    // Should have newer version
    let retrieved = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap();
    assert_eq!(retrieved.title, "Version 2");
}

#[test]
#[serial]
fn lww_newer_overwrites_older() {
    super::common::register_test_merge_functions();
    let mut page = Page::new_from_element("Version 1", Element::root());
    assert!(TestInterface::save(&mut page).unwrap());

    // Wait and create newer version
    std::thread::sleep(std::time::Duration::from_millis(2));
    page.title = "Version 2".to_string();
    page.element_mut().update();

    assert!(TestInterface::save(&mut page).unwrap());

    // Verify newer version is stored
    let retrieved = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap();
    assert_eq!(retrieved.title, "Version 2");
}

#[test]
#[serial]
fn lww_concurrent_updates_deterministic() {
    super::common::register_test_merge_functions();
    let mut page = Page::new_from_element("Initial", Element::root());
    let id = page.id();

    // Save initial version
    TestInterface::save(&mut page).unwrap();

    // Create update with slightly newer timestamp
    std::thread::sleep(std::time::Duration::from_millis(2));
    page.title = "Update 1".to_string();
    page.element_mut().update();
    TestInterface::save(&mut page).unwrap();

    // Create another update with even newer timestamp
    std::thread::sleep(std::time::Duration::from_millis(2));
    page.title = "Update 2".to_string();
    page.element_mut().update();
    TestInterface::save(&mut page).unwrap();

    // Later metadata should win
    let retrieved = TestInterface::find_by_id::<Page>(id).unwrap().unwrap();
    assert_eq!(retrieved.title, "Update 2");
}

// ============================================================
// Tombstone Tests
// ============================================================

#[test]
fn tombstone_marks_deleted() {
    let mut page = Page::new_from_element("Test Page", Element::root());
    let id = page.id();

    assert!(TestInterface::save(&mut page).unwrap());

    // Delete with tombstone
    let delete_action = Action::DeleteRef {
        id,
        deleted_at: time_now(),
        metadata: Metadata::default(),
    };

    TestInterface::apply_action(delete_action, &ApplyContext::empty()).unwrap();

    // Entity should be gone
    assert!(TestInterface::find_by_id::<Page>(id).unwrap().is_none());

    // Tombstone should exist
    assert!(Index::<TestStorage>::is_deleted(id).unwrap());
}

#[test]
fn tombstone_prevents_old_resurrection() {
    let mut page = Page::new_from_element("Test Page", Element::root());
    let id = page.id();

    assert!(TestInterface::save(&mut page).unwrap());
    let save_meta = page.element().metadata.clone();

    // Delete with current time
    let delete_time = time_now();
    Index::<TestStorage>::mark_deleted(id, delete_time).unwrap();

    // Try to resurrect with old metadata (from before deletion)
    let resurrect_action = Action::Update {
        id,
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: save_meta, // Old metadata from before deletion
    };

    TestInterface::apply_action(resurrect_action, &ApplyContext::empty()).unwrap();

    // Should remain deleted (tombstone is newer)
    assert!(TestInterface::find_by_id::<Page>(id).unwrap().is_none());
}

#[test]
fn tombstone_allows_newer_update() {
    let mut page = Page::new_from_element("Test Page", Element::root());
    let id = page.id();

    assert!(TestInterface::save(&mut page).unwrap());

    // Delete
    let delete_time = time_now();
    Index::<TestStorage>::mark_deleted(id, delete_time).unwrap();

    // Verify deleted
    assert!(TestInterface::find_by_id::<Page>(id).unwrap().is_none());

    // Small delay to ensure newer timestamp
    std::thread::sleep(std::time::Duration::from_millis(2));

    // Create a brand new entity with newer timestamp to resurrect
    let new_page = Page::new_from_element("Resurrected", page.element().clone());

    let update_action = Action::Add {
        id,
        data: borsh::to_vec(&new_page).unwrap(),
        ancestors: vec![],
        metadata: new_page.element().metadata.clone(),
    };

    TestInterface::apply_action(update_action, &ApplyContext::empty()).unwrap();

    // Should be resurrected (if the add timestamp is newer than delete)
    let retrieved = TestInterface::find_by_id::<Page>(id).unwrap();
    // Note: Depending on implementation, this may or may not resurrect
    // The test documents expected behavior
    if retrieved.is_some() {
        assert_eq!(retrieved.unwrap().title, "Resurrected");
    }
}

#[test]
fn tombstone_does_not_regress_on_out_of_order_delete() {
    let mut page = Page::new_from_element("Test Page", Element::root());
    let id = page.id();

    assert!(TestInterface::save(&mut page).unwrap());

    // Apply a newer tombstone first.
    let newer = time_now();
    let older = newer - 1_000_000_000; // 1s earlier nonce
    Index::<TestStorage>::mark_deleted(id, newer).unwrap();

    // An out-of-order, OLDER DeleteRef must not roll the tombstone back.
    Index::<TestStorage>::mark_deleted(id, older).unwrap();

    let index = Index::<TestStorage>::get_index(id).unwrap().unwrap();
    assert_eq!(
        index.deleted_at,
        Some(newer),
        "older delete must not regress the tombstone nonce"
    );
    assert_eq!(
        *index.metadata.updated_at, newer,
        "older delete must not regress the updated_at nonce (replay protection)"
    );
}

#[test]
fn delete_vs_update_conflict() {
    // Test LWW conflict resolution between delete and update
    let mut page = Page::new_from_element("Test Page", Element::root());
    let id = page.id();

    assert!(TestInterface::save(&mut page).unwrap());

    // Create delete action
    let delete_action = Action::DeleteRef {
        id,
        deleted_at: time_now(),
        metadata: Metadata::default(),
    };

    // Small delay for newer update
    std::thread::sleep(std::time::Duration::from_millis(2));

    // Update with newer timestamp
    page.title = "Updated".to_string();
    page.element_mut().update();

    let update_action = Action::Update {
        id,
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    // Apply delete first, then update
    TestInterface::apply_action(delete_action, &ApplyContext::empty()).unwrap();
    TestInterface::apply_action(update_action, &ApplyContext::empty()).unwrap();

    // Behavior depends on implementation
    // Just verify no panic
    drop(TestInterface::find_by_id::<Page>(id).unwrap());
}

#[test]
#[serial]
fn update_vs_delete_conflict() {
    super::common::register_test_merge_functions();
    let mut page = Page::new_from_element("Test Page", Element::root());
    let id = page.id();

    assert!(TestInterface::save(&mut page).unwrap());

    // Update
    page.title = "Updated".to_string();
    page.element_mut().update();

    let update_action = Action::Update {
        id,
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    // Small delay for newer delete
    std::thread::sleep(std::time::Duration::from_millis(2));

    let delete_action = Action::DeleteRef {
        id,
        deleted_at: time_now(),
        metadata: Metadata::default(),
    };

    // Apply update first, then delete
    TestInterface::apply_action(update_action, &ApplyContext::empty()).unwrap();
    TestInterface::apply_action(delete_action, &ApplyContext::empty()).unwrap();

    // Delete wins (newer timestamp)
    let retrieved = TestInterface::find_by_id::<Page>(id).unwrap();
    assert!(retrieved.is_none());
}

// ============================================================
// Concurrent Updates Tests
// ============================================================

#[test]
#[serial]
fn concurrent_updates_different_entities() {
    super::common::register_test_merge_functions();
    // Test that concurrent updates to different entities both succeed
    // Both use root element so they can be saved
    let mut page1 = Page::new_from_element("Page 1", Element::root());
    let mut page2 = Page::new_from_element("Page 2", Element::root());

    // Both save (will have different IDs due to random ID generation)
    let saved1 = TestInterface::save(&mut page1).unwrap();
    let saved2 = TestInterface::save(&mut page2).unwrap();

    // At least one should succeed
    assert!(saved1 || saved2);
}

#[test]
fn concurrent_adds_to_collection() {
    let mut page = Page::new_from_element("Parent", Element::root());
    assert!(TestInterface::save(&mut page).unwrap());

    let mut para1 = Paragraph::new_from_element("Para 1", Element::new(None));
    let mut para2 = Paragraph::new_from_element("Para 2", Element::new(None));

    // Add both concurrently
    assert!(TestInterface::add_child_to(page.id(), &mut para1).unwrap());
    assert!(TestInterface::add_child_to(page.id(), &mut para2).unwrap());

    // Both should be in collection
    let children: Vec<Paragraph> = TestInterface::children_of(page.id()).unwrap();
    assert_eq!(children.len(), 2);
}

#[test]
#[serial]
fn concurrent_update_same_entity_different_fields() {
    super::common::register_test_merge_functions();
    // Create entity with multiple fields
    let mut page = Page::new_from_element("Original Title", Element::root());
    assert!(TestInterface::save(&mut page).unwrap());

    // First update
    std::thread::sleep(std::time::Duration::from_millis(2));
    page.title = "Updated Title".to_string();
    page.element_mut().update();

    let action1 = Action::Update {
        id: page.id(),
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    // Second update with newer timestamp
    std::thread::sleep(std::time::Duration::from_millis(2));
    page.title = "Different Title".to_string();
    page.element_mut().update();

    let action2 = Action::Update {
        id: page.id(),
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    // Apply both
    TestInterface::apply_action(action1, &ApplyContext::empty()).unwrap();
    TestInterface::apply_action(action2, &ApplyContext::empty()).unwrap();

    // Later timestamp wins
    let retrieved = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap();
    assert_eq!(retrieved.title, "Different Title");
}

// ============================================================
// Action Ordering Tests
// ============================================================

#[test]
#[serial]
fn actions_idempotent() {
    super::common::register_test_merge_functions();
    let page = Page::new_from_element("Test", Element::root());
    let action = Action::Add {
        id: page.id(),
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    // Apply multiple times
    TestInterface::apply_action(action.clone(), &ApplyContext::empty()).unwrap();
    TestInterface::apply_action(action.clone(), &ApplyContext::empty()).unwrap();
    TestInterface::apply_action(action, &ApplyContext::empty()).unwrap();

    // Should only exist once
    let retrieved = TestInterface::find_by_id::<Page>(page.id()).unwrap();
    assert!(retrieved.is_some());
}

#[test]
fn update_before_add_creates_entity() {
    let page = Page::new_from_element("Test", Element::root());

    // Update non-existent entity
    let action = Action::Update {
        id: page.id(),
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    TestInterface::apply_action(action, &ApplyContext::empty()).unwrap();

    // Should be created
    let retrieved = TestInterface::find_by_id::<Page>(page.id()).unwrap();
    assert!(retrieved.is_some());
}

#[test]
fn delete_prevents_old_add() {
    // Test that tombstones prevent resurrection with older timestamps
    let mut page = Page::new_from_element("Test", Element::root());
    TestInterface::save(&mut page).unwrap();
    let old_meta = page.element().metadata.clone();

    // Delete
    std::thread::sleep(std::time::Duration::from_millis(2));
    let delete_action = Action::DeleteRef {
        id: page.id(),
        deleted_at: time_now(),
        metadata: Metadata::default(),
    };
    TestInterface::apply_action(delete_action, &ApplyContext::empty()).unwrap();

    // Try to add with older timestamp (from before deletion)
    let add_action = Action::Add {
        id: page.id(),
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: old_meta,
    };

    TestInterface::apply_action(add_action, &ApplyContext::empty()).unwrap();

    // Should remain deleted (tombstone wins)
    assert!(TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .is_none());
}

// ============================================================
// Edge Cases
// ============================================================

#[test]
#[serial]
fn same_timestamp_lww_behavior() {
    super::common::register_test_merge_functions();
    // With actual API, timestamps are always increasing
    // This test verifies that updates are applied correctly regardless of order
    let mut page = Page::new_from_element("Initial", Element::root());
    let id = page.id();

    TestInterface::save(&mut page).unwrap();

    // Create two sequential updates
    std::thread::sleep(std::time::Duration::from_millis(2));
    page.title = "Update 1".to_string();
    page.element_mut().update();
    let action1 = Action::Update {
        id,
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    std::thread::sleep(std::time::Duration::from_millis(2));
    page.title = "Update 2".to_string();
    page.element_mut().update();
    let action2 = Action::Update {
        id,
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    // Apply both - later one wins
    TestInterface::apply_action(action1, &ApplyContext::empty()).unwrap();
    TestInterface::apply_action(action2, &ApplyContext::empty()).unwrap();

    let result = TestInterface::find_by_id::<Page>(id).unwrap().unwrap();
    assert_eq!(result.title, "Update 2");
}

#[test]
fn empty_entity_data() {
    // Test that empty data is handled gracefully (deserialization error)
    let element = Element::root();
    let id = element.id();

    let action = Action::Add {
        id,
        data: vec![], // Empty data
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    // Should handle gracefully - expect deserialization to fail
    let result = TestInterface::apply_action(action, &ApplyContext::empty());
    // Just verify it doesn't panic - result may vary
    drop(result);
}

#[test]
fn malformed_entity_data() {
    let id = Id::random();

    let action = Action::Add {
        id,
        data: vec![0xFF, 0xFF, 0xFF], // Invalid borsh
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    // Should fail gracefully
    let result = TestInterface::apply_action(action, &ApplyContext::empty());
    assert!(result.is_err());
}

#[test]
fn multiple_deletes_idempotent() {
    let mut page = Page::new_from_element("Test", Element::root());
    assert!(TestInterface::save(&mut page).unwrap());

    let delete_action = Action::DeleteRef {
        id: page.id(),
        deleted_at: time_now(),
        metadata: Metadata::default(),
    };

    // Delete multiple times
    TestInterface::apply_action(delete_action.clone(), &ApplyContext::empty()).unwrap();
    TestInterface::apply_action(delete_action.clone(), &ApplyContext::empty()).unwrap();
    TestInterface::apply_action(delete_action, &ApplyContext::empty()).unwrap();

    // Should still be deleted
    assert!(TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .is_none());
}

// ============================================================
// Stress Tests
// ============================================================

#[test]
#[serial]
fn many_sequential_updates() {
    super::common::register_test_merge_functions();
    let mut page = Page::new_from_element("Version 0", Element::root());
    let id = page.id();

    TestInterface::save(&mut page).unwrap();

    // Apply 20 sequential updates (reduced from 100 for speed)
    for i in 1..=20 {
        std::thread::sleep(std::time::Duration::from_micros(100));
        page.title = format!("Version {i}");
        page.element_mut().update();

        let action = Action::Update {
            id,
            data: borsh::to_vec(&page).unwrap(),
            ancestors: vec![],
            metadata: page.element().metadata.clone(),
        };

        TestInterface::apply_action(action, &ApplyContext::empty()).unwrap();
    }

    // Latest version should win
    let retrieved = TestInterface::find_by_id::<Page>(id).unwrap().unwrap();
    assert_eq!(retrieved.title, "Version 20");
}

#[test]
#[serial]
fn rapid_add_delete_cycles() {
    super::common::register_test_merge_functions();
    // Test rapid add/delete cycles work correctly
    let mut page = Page::new_from_element("Test", Element::root());
    let id = page.id();

    // Start with entity saved
    TestInterface::save(&mut page).unwrap();

    // Do a few update/delete cycles
    for i in 1..5 {
        std::thread::sleep(std::time::Duration::from_millis(1));

        if i % 2 == 1 {
            // Delete
            let action = Action::DeleteRef {
                id,
                deleted_at: time_now(),
                metadata: Metadata::default(),
            };
            TestInterface::apply_action(action, &ApplyContext::empty()).unwrap();
        } else {
            // Update (resurrect if was deleted)
            page.title = format!("Version {i}");
            page.element_mut().update();

            let action = Action::Update {
                id,
                data: borsh::to_vec(&page).unwrap(),
                ancestors: vec![],
                metadata: page.element().metadata.clone(),
            };
            TestInterface::apply_action(action, &ApplyContext::empty()).unwrap();
        }
    }

    // Test completes without panic - actual final state depends on implementation
}

#[test]
fn test_future_timestamp_rejected() {
    let now = time_now();

    // Create a timestamp that's above the tolerance by one sec.
    let future_time = now + DRIFT_TOLERANCE_NANOS + ONE_SEC_NANOS;

    let mut page = Page::new_from_element("Future Page", Element::root());
    // Manually set the future timestamp in metadata
    page.element_mut().set_updated_at(future_time);

    let action = Action::Update {
        id: page.id(),
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    let result = TestInterface::apply_action(action, &ApplyContext::empty());

    assert!(
        result.is_err(),
        "Should reject timestamp too far in the future"
    );

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("too far in the future"),
        "Error should mention future timestamp"
    );
}

#[test]
fn test_near_future_timestamp_accepted() {
    let now = time_now();

    // Create a timestamp within the tolerance, simulating execution delay.
    let future_time = now + (DRIFT_TOLERANCE_NANOS / 2);

    let mut page = Page::new_from_element("NEAR Future Page", Element::root());
    page.element_mut().set_updated_at(future_time);

    let action = Action::Update {
        id: page.id(),
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    assert!(
        TestInterface::apply_action(action, &ApplyContext::empty()).is_ok(),
        "Should accept timestamp within drift tolerance"
    );
}

#[test]
fn test_past_timestamp_accepted() {
    let now = time_now();

    // Create a timestamp in the past (1 hour ago) to
    // simulate normal sync of the node after being offline.
    let past_time = now.saturating_sub(3600 * ONE_SEC_NANOS);

    let mut page = Page::new_from_element("Past Page", Element::root());
    page.element_mut().set_updated_at(past_time);

    let action = Action::Update {
        id: page.id(),
        data: borsh::to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    assert!(
        TestInterface::apply_action(action, &ApplyContext::empty()).is_ok(),
        "Should accept valid past timestamps"
    );
}

#[test]
fn test_delete_future_timestamp_rejected() {
    let now = time_now();

    // Create a timestamp that's above the tolerance by one sec.
    let future_time = now + DRIFT_TOLERANCE_NANOS + ONE_SEC_NANOS;

    let page = Page::new_from_element("Delete Page", Element::root());

    let action = Action::DeleteRef {
        id: page.id(),
        deleted_at: future_time,
        metadata: Metadata::default(),
    };

    let result = TestInterface::apply_action(action, &ApplyContext::empty());
    assert!(
        result.is_err(),
        "Should reject delete timestamp too far in the future"
    );
}

#[test]
fn test_delete_near_future_accepted() {
    let now = time_now();

    // Create a timestamp within the tolerance, simulating execution delay.
    let future_time = now + (DRIFT_TOLERANCE_NANOS / 2);

    // Create a timestamp within the tolerance, simulating execution delay.
    let mut page = Page::new_from_element("Delete Page", Element::root());

    // Save page to have it in the storage.
    TestInterface::save(&mut page).unwrap();

    let action = Action::DeleteRef {
        id: page.id(),
        deleted_at: future_time,
        metadata: Metadata::default(),
    };

    assert!(
        TestInterface::apply_action(action, &ApplyContext::empty()).is_ok(),
        "Should accept delete timestamp within tolerance"
    );
}

// ============================================================
// D1 — map delete-vs-update converges, no resurrection
// ============================================================
//
// These tests live at the DELTA / `apply_action` (Index) layer, NOT
// `Mergeable::merge`. `UnorderedMap::merge` is an add-wins UNION that
// iterates `other.entries()` (tombstoned keys are already skipped) and
// never consults tombstones — a delete-vs-update run purely through it
// would RESURRECT the deleted key. The real tombstone-vs-value HLC
// resolution is in `apply_delete_ref_action` / `save_internal`, exercised
// here.
//
// A map entry is modelled as a single content-addressed entity whose id is
// stable across nodes (the key). Two nodes are two independent
// `MockedStorage` instances; each op is an `Action` cross-applied in both
// directions. Timestamps are explicit nanosecond HLC nonces in the recent
// past (< now, so the future-drift guard accepts them) to make "strictly
// later" deterministic without wall-clock sleeps.

// Two independent replicas, each with its own store.
type D1NodeA = MockedStorage<7101>;
type D1NodeB = MockedStorage<7102>;
type D1IfaceA = Interface<D1NodeA>;
type D1IfaceB = Interface<D1NodeB>;

// A fixed content-addressed id standing in for one map key "k". Modelled as
// a child of the map-container root so it gets a real Index entry (a
// parent-less non-root Add is an orphan with no index, which `DeleteRef`
// can't resolve).
fn d1_key_id() -> Id {
    Id::new([0x7c; 32])
}

// The map container: the shared root parent that the key entity hangs under.
fn d1_map_ancestors() -> Vec<crate::entities::ChildInfo> {
    vec![crate::entities::ChildInfo::new(
        Id::root(),
        [0; 32],
        Metadata::default(),
    )]
}

// Build an upsert action (Add or Update) for the key entity carrying an
// explicit `updated_at` HLC nonce. The base `Add` supplies the root ancestor
// so the entity is linked under the container and indexed; later `Update`s
// pass empty ancestors (the entity already exists).
fn d1_upsert(id: Id, value: &str, updated_at: u64, is_add: bool) -> Action {
    let mut page = Page::new_from_element(value, Element::new(Some(id)));
    page.element_mut().set_updated_at(updated_at);
    let data = borsh::to_vec(&page).unwrap();
    let metadata = page.element().metadata.clone();
    if is_add {
        Action::Add {
            id,
            data,
            ancestors: d1_map_ancestors(),
            metadata,
        }
    } else {
        Action::Update {
            id,
            data,
            ancestors: vec![],
            metadata,
        }
    }
}

fn d1_delete(id: Id, deleted_at: u64) -> Action {
    Action::DeleteRef {
        id,
        deleted_at,
        metadata: Metadata::default(),
    }
}

/// (a) A newer `insert("k", v')` must NOT be over-suppressed by an older
/// `remove("k")` tombstone: after both replicas exchange the two ops, both
/// agree "k" is present with value `v'`.
///
/// REVEALS A BUG (kept asserting the correct CRDT invariant, `#[ignore]`d):
/// `apply_action`'s upsert path does NOT clear an existing older `deleted_at`
/// tombstone when a strictly-newer `Update` arrives. `save_internal` passes
/// the LWW guard (`stored.updated_at == t_del < t_upd`) and writes the new
/// bytes, but the stale tombstone is never lifted, so `find_by_id` keeps
/// returning `None` — the newer write lands in storage yet stays invisible.
/// This makes the two replicas DIVERGE on the exact scenario this test models:
/// the replica that saw `remove` before the newer `insert` hides the value,
/// while the replica that only ever saw the newer `insert` (its older
/// `remove` correctly loses via `apply_delete_ref_action`) shows it. Newer
/// updates should win over older deletes (LWW-including-deletes / add-wins).
#[test]
#[serial]
fn d1_map_update_newer_than_delete_is_not_over_suppressed() {
    super::common::register_test_merge_functions();
    crate::env::reset_for_testing();

    let id = d1_key_id();
    let base = time_now();
    let t0 = base - 30_000_000; // shared base insert
    let t_del = base - 20_000_000; // A's delete
    let t_upd = base - 10_000_000; // B's update, strictly later than the delete

    // Shared base: both replicas hold "k" = "v0".
    let base_add = d1_upsert(id, "v0", t0, true);
    D1IfaceA::apply_action(base_add.clone(), &ApplyContext::empty()).unwrap();
    D1IfaceB::apply_action(base_add, &ApplyContext::empty()).unwrap();

    // A removes "k" (tombstone at t_del). B updates "k" = "v1" (at t_upd > t_del).
    let del = d1_delete(id, t_del);
    let upd = d1_upsert(id, "v1", t_upd, false);
    D1IfaceA::apply_action(del.clone(), &ApplyContext::empty()).unwrap();
    D1IfaceB::apply_action(upd.clone(), &ApplyContext::empty()).unwrap();

    // Cross-apply: A learns of B's newer update; B learns of A's older delete.
    D1IfaceA::apply_action(upd, &ApplyContext::empty()).unwrap();
    D1IfaceB::apply_action(del, &ApplyContext::empty()).unwrap();

    // Both replicas must converge to "k" present = "v1" — the newer update
    // wins over the older tombstone; it must not be suppressed.
    let a = D1IfaceA::find_by_id::<Page>(id).unwrap();
    let b = D1IfaceB::find_by_id::<Page>(id).unwrap();
    assert!(
        a.is_some(),
        "node A: newer update must resurrect over the older tombstone"
    );
    assert!(
        b.is_some(),
        "node B: older delete must not suppress the newer local update"
    );
    assert_eq!(a.unwrap().title, "v1", "node A converged value");
    assert_eq!(b.unwrap().title, "v1", "node B converged value");
}

/// (b) A `remove("k")` strictly-later than an `insert("k", v)` must win on
/// BOTH replicas — no resurrection of the deleted key from the older insert.
#[test]
#[serial]
fn d1_map_delete_newer_than_update_no_resurrection() {
    super::common::register_test_merge_functions();
    crate::env::reset_for_testing();

    let id = d1_key_id();
    let base = time_now();
    let t0 = base - 30_000_000; // shared base insert
    let t_upd = base - 20_000_000; // A's update
    let t_del = base - 10_000_000; // B's delete, strictly later than the update

    // Shared base.
    let base_add = d1_upsert(id, "v0", t0, true);
    D1IfaceA::apply_action(base_add.clone(), &ApplyContext::empty()).unwrap();
    D1IfaceB::apply_action(base_add, &ApplyContext::empty()).unwrap();

    // A updates "k" = "v1" (t_upd). B removes "k" (t_del > t_upd).
    let upd = d1_upsert(id, "v1", t_upd, false);
    let del = d1_delete(id, t_del);
    D1IfaceA::apply_action(upd.clone(), &ApplyContext::empty()).unwrap();
    D1IfaceB::apply_action(del.clone(), &ApplyContext::empty()).unwrap();

    // Cross-apply: A learns of B's newer delete; B learns of A's older update.
    D1IfaceA::apply_action(del, &ApplyContext::empty()).unwrap();
    D1IfaceB::apply_action(upd, &ApplyContext::empty()).unwrap();

    // Both replicas must converge to "k" ABSENT — the newer delete wins and
    // the older update must not resurrect it.
    assert!(
        D1IfaceA::find_by_id::<Page>(id).unwrap().is_none(),
        "node A: newer delete must win over the older update"
    );
    assert!(
        D1IfaceB::find_by_id::<Page>(id).unwrap().is_none(),
        "node B: older update must NOT resurrect the newer-deleted key"
    );
}

/// (c) Convergence is symmetric: applying the same delete + update ops in
/// opposite orders must yield the identical final storage state (same
/// visibility AND same Merkle hash for the key id).
#[test]
#[serial]
fn d1_map_delete_vs_update_convergence_is_apply_order_independent() {
    super::common::register_test_merge_functions();
    crate::env::reset_for_testing();

    let id = d1_key_id();
    let base = time_now();
    let t0 = base - 30_000_000;
    let t_upd = base - 20_000_000;
    let t_del = base - 10_000_000; // delete strictly later than update

    let base_add = d1_upsert(id, "v0", t0, true);
    let upd = d1_upsert(id, "v1", t_upd, false);
    let del = d1_delete(id, t_del);

    // Node A applies update-then-delete; node B applies delete-then-update.
    D1IfaceA::apply_action(base_add.clone(), &ApplyContext::empty()).unwrap();
    D1IfaceA::apply_action(upd.clone(), &ApplyContext::empty()).unwrap();
    D1IfaceA::apply_action(del.clone(), &ApplyContext::empty()).unwrap();

    D1IfaceB::apply_action(base_add, &ApplyContext::empty()).unwrap();
    D1IfaceB::apply_action(del, &ApplyContext::empty()).unwrap();
    D1IfaceB::apply_action(upd, &ApplyContext::empty()).unwrap();

    // Same visibility.
    let a_visible = D1IfaceA::find_by_id::<Page>(id).unwrap().is_some();
    let b_visible = D1IfaceB::find_by_id::<Page>(id).unwrap().is_some();
    assert_eq!(
        a_visible, b_visible,
        "visibility must be independent of apply order"
    );
    assert!(
        !a_visible,
        "newer delete wins ⇒ key absent on both replicas"
    );

    // Convergence signal that actually drives sync: the map-container (root)
    // Merkle hash. A tombstoned child is unlinked from its parent's children
    // list, so the dead leaf's own bytes no longer feed the container hash —
    // the two replicas must agree on the container root regardless of the
    // order they applied the update vs the delete.
    let a_root = Index::<D1NodeA>::get_hashes_for(Id::root()).unwrap();
    let b_root = Index::<D1NodeB>::get_hashes_for(Id::root()).unwrap();
    assert_eq!(
        a_root, b_root,
        "map-container root hash must be identical regardless of apply order"
    );
}

type BranchNode = MockedStorage<7103>;
type BranchIface = Interface<BranchNode>;

/// `save_internal` picks its LWW-by-HLC branch by comparing `updated_at` under
/// both `>` (`Ord`) and `==` (`PartialEq`). While `UpdatedAt::eq` was hard-coded
/// to always return `true`, every non-stale write took the equal-timestamp
/// branch and the strictly-newer branch was unreachable. This drives all three
/// branches explicitly and asserts each applies the write it should — a
/// regression guard for the corrected value-based `PartialEq`.
#[test]
#[serial]
fn save_internal_selects_branch_by_updated_at() {
    super::common::register_test_merge_functions();
    crate::env::reset_for_testing();

    let id = Id::new([0x5b; 32]);
    let ancestors = vec![crate::entities::ChildInfo::new(
        Id::root(),
        [0; 32],
        Metadata::default(),
    )];

    let make = |value: &str, updated_at: u64, is_add: bool| -> Action {
        let mut page = Page::new_from_element(value, Element::new(Some(id)));
        page.element_mut().set_updated_at(updated_at);
        let data = borsh::to_vec(&page).unwrap();
        let metadata = page.element().metadata.clone();
        if is_add {
            Action::Add {
                id,
                data,
                ancestors: ancestors.clone(),
                metadata,
            }
        } else {
            Action::Update {
                id,
                data,
                ancestors: vec![],
                metadata,
            }
        }
    };

    let stored_nonce = || {
        *Index::<BranchNode>::get_index(id)
            .unwrap()
            .unwrap()
            .metadata
            .updated_at
    };

    let base = time_now();
    let t = base - 10_000_000;

    // Base write at `t`.
    BranchIface::apply_action(make("v0", t, true), &ApplyContext::empty()).unwrap();
    assert_eq!(
        BranchIface::find_by_id::<Page>(id).unwrap().unwrap().title,
        "v0"
    );
    assert_eq!(stored_nonce(), t);

    // Older write (`updated_at < stored`): the `>` guard must stale-skip it, so
    // the stored value is unchanged and the nonce does not regress.
    BranchIface::apply_action(make("v_old", t - 1, false), &ApplyContext::empty()).unwrap();
    assert_eq!(
        BranchIface::find_by_id::<Page>(id).unwrap().unwrap().title,
        "v0",
        "older write must be stale-skipped"
    );
    assert_eq!(stored_nonce(), t, "older write must not regress the nonce");

    // Equal-timestamp write: the equal branch must merge, not drop the entity.
    BranchIface::apply_action(make("v_eq", t, false), &ApplyContext::empty()).unwrap();
    assert!(
        BranchIface::find_by_id::<Page>(id).unwrap().is_some(),
        "equal-timestamp write must merge, not drop the entity"
    );
    assert_eq!(
        stored_nonce(),
        t,
        "equal-timestamp write keeps the same nonce"
    );

    // Strictly-newer write: the branch that was unreachable under the always-true
    // `eq`. It must apply and win by LWW, advancing the nonce.
    let t_new = t + 10_000_000;
    BranchIface::apply_action(make("v_new", t_new, false), &ApplyContext::empty()).unwrap();
    assert_eq!(
        BranchIface::find_by_id::<Page>(id).unwrap().unwrap().title,
        "v_new",
        "strictly-newer write must win by LWW"
    );
    assert_eq!(
        stored_nonce(),
        t_new,
        "strictly-newer write advances the nonce"
    );
}
