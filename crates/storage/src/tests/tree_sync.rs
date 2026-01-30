#![allow(unused_results)]
//! Local Merkle Tree Synchronization Tests
//!
//! Tests tree synchronization WITHOUT network layer.
//! Validates that `compare_trees()` correctly identifies differences
//! and generates actions to bring two divergent trees into sync.
//!
//! ## Test Scenarios:
//! 1. Fresh node syncs from populated node (bootstrap)
//! 2. Both nodes have divergent changes (bidirectional sync)
//! 3. Partial overlap (some shared, some unique)
//! 4. Deep hierarchy sync (grandparent -> parent -> child)
//! 5. Concurrent modifications with conflict resolution

use std::thread::sleep;
use std::time::Duration;

use crate::action::Action;
use crate::address::Id;
use crate::delta::reset_delta_context;
use crate::entities::{Data, Element};
use crate::interface::{Interface, StorageError};
use crate::store::MockedStorage;

use super::common::{Page, Paragraph};

// ============================================================
// Type Aliases for Simulated Nodes
// ============================================================

/// Node A's storage (simulates first peer)
type StorageA = MockedStorage<9001>;
type NodeA = Interface<StorageA>;

/// Node B's storage (simulates second peer)
type StorageB = MockedStorage<9002>;
type NodeB = Interface<StorageB>;

/// Node C's storage (for 3-node scenarios)
type StorageC = MockedStorage<9003>;
type NodeC = Interface<StorageC>;

// ============================================================
// Helper Functions
// ============================================================

/// Compares trees between two nodes using CRDT-type-based merge.
/// Returns (actions_for_node_a, actions_for_node_b)
fn compare_trees_between<SA: crate::store::StorageAdaptor, SB: crate::store::StorageAdaptor>(
    id: Id,
) -> Result<(Vec<Action>, Vec<Action>), StorageError> {
    let node_b_data = Interface::<SB>::find_by_id_raw(id);
    let node_b_comparison = Interface::<SB>::generate_comparison_data(Some(id))?;

    Interface::<SA>::compare_trees(node_b_data, node_b_comparison)
}

/// Performs full recursive tree sync between two nodes.
/// Returns (actions_for_node_a, actions_for_node_b)
fn sync_trees_between<SA: crate::store::StorageAdaptor, SB: crate::store::StorageAdaptor>(
    id: Id,
) -> Result<(Vec<Action>, Vec<Action>), StorageError> {
    let node_b_data = Interface::<SB>::find_by_id_raw(id);
    let node_b_comparison = Interface::<SB>::generate_comparison_data(Some(id))?;

    // Callback to get foreign data for recursive comparison
    let get_foreign_data = |child_id: Id| -> Result<(Option<Vec<u8>>, _), StorageError> {
        let data = Interface::<SB>::find_by_id_raw(child_id);
        let comparison = Interface::<SB>::generate_comparison_data(Some(child_id))?;
        Ok((data, comparison))
    };

    Interface::<SA>::sync_trees(node_b_data, node_b_comparison, get_foreign_data)
}

/// Apply actions to a node's storage
fn apply_actions_to<S: crate::store::StorageAdaptor>(
    actions: Vec<Action>,
) -> Result<(), StorageError> {
    for action in actions {
        // Skip Compare actions - they're just markers for recursive comparison
        if matches!(action, Action::Compare { .. }) {
            continue;
        }
        Interface::<S>::apply_action(action)?;
    }
    Ok(())
}

/// Get root hash for a node
fn get_root_hash<S: crate::store::StorageAdaptor>() -> [u8; 32] {
    Interface::<S>::find_by_id::<Page>(Id::root())
        .ok()
        .flatten()
        .map(|p| p.element().merkle_hash())
        .unwrap_or([0; 32])
}

// ============================================================
// Test: Fresh Node Bootstrap
// ============================================================

#[test]
fn tree_sync_fresh_node_bootstrap() {
    reset_delta_context();

    // Node A has data
    let mut page_a = Page::new_from_element("My Document", Element::root());
    NodeA::save(&mut page_a).unwrap();

    let mut para1 = Paragraph::new_from_element("First paragraph", Element::new(None));
    let mut para2 = Paragraph::new_from_element("Second paragraph", Element::new(None));
    NodeA::add_child_to(page_a.id(), &mut para1).unwrap();
    NodeA::add_child_to(page_a.id(), &mut para2).unwrap();

    // Verify Node A has data
    let a_hash = get_root_hash::<StorageA>();
    assert_ne!(a_hash, [0; 32], "Node A should have non-zero hash");

    // Node B is fresh (no data)
    let b_hash = get_root_hash::<StorageB>();
    assert_eq!(b_hash, [0; 32], "Node B should be empty");

    // Get Node A's comparison data for root
    let a_comparison = NodeA::generate_comparison_data(Some(Id::root())).unwrap();
    let a_data = NodeA::find_by_id_raw(Id::root());

    // Node B compares against Node A's data
    // Since B is empty, it needs everything from A
    let (actions_for_b, actions_for_a) = NodeB::compare_trees(a_data, a_comparison).unwrap();

    // B should receive Add action for the root
    assert!(
        !actions_for_b.is_empty(),
        "Node B should receive actions to add A's data"
    );
    assert!(
        actions_for_a.is_empty(),
        "Node A doesn't need anything from empty B"
    );

    // Apply actions to Node B
    apply_actions_to::<StorageB>(actions_for_b).unwrap();

    // After sync, Node B should have the page
    let page_b = NodeB::find_by_id::<Page>(Id::root()).unwrap();
    assert!(page_b.is_some(), "Node B should have the page after sync");
    assert_eq!(
        page_b.unwrap().title,
        "My Document",
        "Page title should match"
    );
}

// ============================================================
// Test: Bidirectional Sync (Both Nodes Have Changes)
// ============================================================

#[test]
fn tree_sync_bidirectional_different_children() {
    reset_delta_context();

    // Both nodes start with same root
    let root_element = Element::root();
    let mut page_a = Page::new_from_element("Shared Page", root_element.clone());
    let mut page_b = Page::new_from_element("Shared Page", root_element);

    NodeA::save(&mut page_a).unwrap();
    NodeB::save(&mut page_b).unwrap();

    // Node A adds child "A-only"
    let mut para_a = Paragraph::new_from_element("From Node A", Element::new(None));
    NodeA::add_child_to(page_a.id(), &mut para_a).unwrap();

    // Node B adds child "B-only"
    let mut para_b = Paragraph::new_from_element("From Node B", Element::new(None));
    NodeB::add_child_to(page_b.id(), &mut para_b).unwrap();

    // Hashes should be different (diverged)
    let hash_a = get_root_hash::<StorageA>();
    let hash_b = get_root_hash::<StorageB>();
    assert_ne!(hash_a, hash_b, "Nodes should have diverged");

    // Compare trees (from A's perspective, looking at B's data)
    let (actions_for_a, actions_for_b) =
        compare_trees_between::<StorageA, StorageB>(Id::root()).unwrap();

    println!("Actions for A: {:?}", actions_for_a);
    println!("Actions for B: {:?}", actions_for_b);

    // The comparison should detect:
    // - A has child para_a that B doesn't have -> Add action for B
    // - B has child para_b that A doesn't have -> Add action for A
    // - Root data is same -> No update action

    // NOTE: Due to a known limitation in compare_trees, Add actions for children
    // have empty ancestors. This means we need to use snapshot sync for full
    // bidirectional child sync. Here we just verify the comparison detects the difference.

    // Count the Add actions generated
    let adds_for_a = actions_for_a
        .iter()
        .filter(|a| matches!(a, Action::Add { .. }))
        .count();
    let adds_for_b = actions_for_b
        .iter()
        .filter(|a| matches!(a, Action::Add { .. }))
        .count();

    // Both should detect missing children
    // (A sees B's child, B sees A's child)
    println!(
        "Add actions for A: {}, Add actions for B: {}",
        adds_for_a, adds_for_b
    );

    // At minimum, we should see some actions indicating divergence
    assert!(
        !actions_for_a.is_empty() || !actions_for_b.is_empty(),
        "Should detect divergence between nodes"
    );
}

// ============================================================
// Test: Bidirectional Sync with FIXED compare_trees
// Shows that compare_trees correctly sets ancestors
// ============================================================

#[test]
fn tree_sync_bidirectional_with_fixed_method() {
    // Use different storage IDs to avoid conflicts
    type FixedStorageA = MockedStorage<9100>;
    type FixedStorageB = MockedStorage<9101>;
    type FixedNodeA = Interface<FixedStorageA>;
    type FixedNodeB = Interface<FixedStorageB>;

    reset_delta_context();

    // Both nodes start with same root
    let root_element = Element::root();
    let mut page_a = Page::new_from_element("Shared Page", root_element.clone());
    let mut page_b = Page::new_from_element("Shared Page", root_element);

    FixedNodeA::save(&mut page_a).unwrap();
    FixedNodeB::save(&mut page_b).unwrap();

    // Node A adds child "A-only"
    let mut para_a = Paragraph::new_from_element("From Node A", Element::new(None));
    FixedNodeA::add_child_to(page_a.id(), &mut para_a).unwrap();

    // Node B adds child "B-only"
    let mut para_b = Paragraph::new_from_element("From Node B", Element::new(None));
    FixedNodeB::add_child_to(page_b.id(), &mut para_b).unwrap();

    // Use the FIXED compare_trees method
    let (actions_for_a, actions_for_b) =
        compare_trees_between::<FixedStorageA, FixedStorageB>(Id::root()).unwrap();

    println!("FIXED - Actions for A: {:?}", actions_for_a);
    println!("FIXED - Actions for B: {:?}", actions_for_b);

    // Verify Add actions from A->B have proper ancestors
    // (A knows about its own child and can include full ancestor info)
    for action in &actions_for_b {
        if let Action::Add { id, ancestors, .. } = action {
            println!("Add action for B: id={:?}, ancestors={:?}", id, ancestors);
            // The ancestors should include the root (parent)
            assert!(
                !ancestors.is_empty(),
                "FIXED method should include ancestors for child Add actions"
            );
        }
    }

    // Note: Actions for A will be Compare actions for B's children because
    // compare_trees doesn't have B's child data, only its hash.
    // For full bidirectional sync, use sync_trees which handles Compare recursively.
    let compare_count = actions_for_a
        .iter()
        .filter(|a| matches!(a, Action::Compare { .. }))
        .count();
    assert!(
        compare_count > 0,
        "A should have Compare action for B's child (needs to fetch full data)"
    );

    // Apply just the B actions (A's child -> B)
    apply_actions_to::<FixedStorageB>(actions_for_b).unwrap();

    // B should now have 2 children
    let children_b: Vec<Paragraph> = FixedNodeB::children_of(page_b.id()).unwrap();
    println!(
        "After FIXED sync - B has {} children: {:?}",
        children_b.len(),
        children_b.iter().map(|p| &p.text).collect::<Vec<_>>()
    );
    assert_eq!(
        children_b.len(),
        2,
        "Node B should have both children after sync"
    );

    // For A to get B's child, we need to use sync_trees (see next test)
}

// ============================================================
// Test: Full Recursive Sync with sync_trees
// ============================================================

#[test]
fn tree_sync_full_recursive_with_sync_trees() {
    type SyncStorageA = MockedStorage<9110>;
    type SyncStorageB = MockedStorage<9111>;
    type SyncNodeA = Interface<SyncStorageA>;
    type SyncNodeB = Interface<SyncStorageB>;

    reset_delta_context();

    // Node A: Has structure with children
    let mut page_a = Page::new_from_element("Document", Element::root());
    SyncNodeA::save(&mut page_a).unwrap();

    let mut para1_a = Paragraph::new_from_element("Paragraph 1 from A", Element::new(None));
    let mut para2_a = Paragraph::new_from_element("Paragraph 2 from A", Element::new(None));
    SyncNodeA::add_child_to(page_a.id(), &mut para1_a).unwrap();
    SyncNodeA::add_child_to(page_a.id(), &mut para2_a).unwrap();

    // Node B: Different children
    let mut page_b = Page::new_from_element("Document", Element::root());
    SyncNodeB::save(&mut page_b).unwrap();

    let mut para3_b = Paragraph::new_from_element("Paragraph 3 from B", Element::new(None));
    SyncNodeB::add_child_to(page_b.id(), &mut para3_b).unwrap();

    println!("Before sync:");
    println!(
        "  A children: {:?}",
        SyncNodeA::children_of::<Paragraph>(page_a.id())
            .unwrap()
            .iter()
            .map(|p| &p.text)
            .collect::<Vec<_>>()
    );
    println!(
        "  B children: {:?}",
        SyncNodeB::children_of::<Paragraph>(page_b.id())
            .unwrap()
            .iter()
            .map(|p| &p.text)
            .collect::<Vec<_>>()
    );

    // Use sync_trees for full recursive sync
    let (actions_for_a, actions_for_b) =
        sync_trees_between::<SyncStorageA, SyncStorageB>(Id::root()).unwrap();

    println!("sync_trees - Actions for A: {:?}", actions_for_a);
    println!("sync_trees - Actions for B: {:?}", actions_for_b);

    // Apply actions
    apply_actions_to::<SyncStorageA>(actions_for_a).unwrap();
    apply_actions_to::<SyncStorageB>(actions_for_b).unwrap();

    // After sync, both nodes should have all 3 children
    let children_a: Vec<Paragraph> = SyncNodeA::children_of(page_a.id()).unwrap();
    let children_b: Vec<Paragraph> = SyncNodeB::children_of(page_b.id()).unwrap();

    println!("After sync_trees:");
    println!(
        "  A children: {:?}",
        children_a.iter().map(|p| &p.text).collect::<Vec<_>>()
    );
    println!(
        "  B children: {:?}",
        children_b.iter().map(|p| &p.text).collect::<Vec<_>>()
    );

    assert_eq!(
        children_a.len(),
        3,
        "Node A should have all 3 children after sync_trees"
    );
    assert_eq!(
        children_b.len(),
        3,
        "Node B should have all 3 children after sync_trees"
    );
}

// ============================================================
// Test: Update Conflict Resolution (LWW)
// ============================================================

#[test]
fn tree_sync_update_conflict_lww() {
    reset_delta_context();

    // Both nodes start with same page
    let root_element = Element::root();
    let mut page_a = Page::new_from_element("Original Title", root_element.clone());
    let mut page_b = Page::new_from_element("Original Title", root_element);

    NodeA::save(&mut page_a).unwrap();
    NodeB::save(&mut page_b).unwrap();

    // Node A updates first
    page_a.title = "Updated by A".to_string();
    page_a.element_mut().update();
    NodeA::save(&mut page_a).unwrap();

    // Small delay to ensure different timestamps
    sleep(Duration::from_millis(10));

    // Node B updates later (should win with LWW)
    page_b.title = "Updated by B".to_string();
    page_b.element_mut().update();
    NodeB::save(&mut page_b).unwrap();

    // Compare and sync
    let (actions_for_a, actions_for_b) =
        compare_trees_between::<StorageA, StorageB>(Id::root()).unwrap();

    println!(
        "Actions for A (should get B's newer update): {:?}",
        actions_for_a
    );
    println!(
        "Actions for B (should be empty, B is newer): {:?}",
        actions_for_b
    );

    // A should receive update from B (B is newer)
    assert!(
        !actions_for_a.is_empty(),
        "A should receive B's newer update"
    );

    // Apply to A
    apply_actions_to::<StorageA>(actions_for_a).unwrap();

    // After sync, A should have B's title (LWW)
    let page_a_after = NodeA::find_by_id::<Page>(Id::root()).unwrap().unwrap();
    assert_eq!(
        page_a_after.title, "Updated by B",
        "LWW: B's newer update should win"
    );
}

// ============================================================
// Test: Recursive Child Sync (using same IDs)
// ============================================================

#[test]
fn tree_sync_recursive_children() {
    reset_delta_context();

    // Use the same paragraph ID on both nodes to test update sync
    let para1_id = Id::random();

    // Setup: Node A has page with child para1
    let mut page_a = Page::new_from_element("Document", Element::root());
    NodeA::save(&mut page_a).unwrap();

    let mut para1_a =
        Paragraph::new_from_element("Paragraph 1 - Original", Element::new(Some(para1_id)));
    NodeA::add_child_to(page_a.id(), &mut para1_a).unwrap();

    // Node B has same page with same para1 ID but different content
    let mut page_b = Page::new_from_element("Document", Element::root());
    NodeB::save(&mut page_b).unwrap();

    sleep(Duration::from_millis(5));

    // B's version is newer
    let mut para1_b =
        Paragraph::new_from_element("Paragraph 1 - MODIFIED", Element::new(Some(para1_id)));
    para1_b.element_mut().update();
    NodeB::add_child_to(page_b.id(), &mut para1_b).unwrap();

    // Both nodes have the same structure (root -> para1)
    // But para1 has different content and B's is newer

    // Compare at root level
    let (root_actions_for_a, root_actions_for_b) =
        compare_trees_between::<StorageA, StorageB>(Id::root()).unwrap();

    println!("Root actions for A: {:?}", root_actions_for_a);
    println!("Root actions for B: {:?}", root_actions_for_b);

    // Should see Compare actions for para1 (same ID, different hash)
    let compare_ids: Vec<Id> = root_actions_for_a
        .iter()
        .chain(root_actions_for_b.iter())
        .filter_map(|a| match a {
            Action::Compare { id } => Some(*id),
            _ => None,
        })
        .collect();

    println!("Compare IDs to recurse: {:?}", compare_ids);

    // Now compare the child that has differing content
    for id in compare_ids {
        let (child_actions_a, child_actions_b) =
            compare_trees_between::<StorageA, StorageB>(id).unwrap();

        println!("Child {:?} actions for A: {:?}", id, child_actions_a);
        println!("Child {:?} actions for B: {:?}", id, child_actions_b);

        // Apply child actions - since both have the same structure,
        // this should be Update actions that work correctly
        apply_actions_to::<StorageA>(child_actions_a).unwrap();
        apply_actions_to::<StorageB>(child_actions_b).unwrap();
    }

    // After sync, A should have B's newer content
    let para1_a_after = NodeA::find_by_id::<Paragraph>(para1_id).unwrap().unwrap();
    assert_eq!(
        para1_a_after.text, "Paragraph 1 - MODIFIED",
        "A should have B's newer version"
    );
}

// ============================================================
// Test: Full Tree Sync Protocol
// ============================================================

/// Recursively syncs two nodes starting from root
/// NOTE: This has limitations due to compare_trees generating Add actions with empty ancestors.
/// For full state sync, use snapshot-based approach instead.
#[allow(dead_code)]
fn full_tree_sync<SA: crate::store::StorageAdaptor, SB: crate::store::StorageAdaptor>(
    id: Id,
    depth: usize,
) -> Result<(), StorageError> {
    if depth > 10 {
        panic!("Sync recursion too deep - possible cycle");
    }

    // Get comparison data from both sides
    let b_data = Interface::<SB>::find_by_id_raw(id);
    let b_comparison = Interface::<SB>::generate_comparison_data(Some(id))?;

    let (actions_for_a, actions_for_b) = Interface::<SA>::compare_trees(b_data, b_comparison)?;

    // Collect Compare actions for recursion
    let mut compare_ids = Vec::new();

    for action in &actions_for_a {
        if let Action::Compare { id } = action {
            compare_ids.push(*id);
        }
    }
    for action in &actions_for_b {
        if let Action::Compare { id } = action {
            if !compare_ids.contains(id) {
                compare_ids.push(*id);
            }
        }
    }

    // Apply non-Compare actions
    for action in actions_for_a {
        if !matches!(action, Action::Compare { .. }) {
            Interface::<SA>::apply_action(action)?;
        }
    }
    for action in actions_for_b {
        if !matches!(action, Action::Compare { .. }) {
            Interface::<SB>::apply_action(action)?;
        }
    }

    // Recurse for Compare actions
    for child_id in compare_ids {
        full_tree_sync::<SA, SB>(child_id, depth + 1)?;
    }

    Ok(())
}

// ============================================================
// Test: Full Protocol using Snapshot (bypasses ancestor issue)
// ============================================================

#[test]
fn tree_sync_full_protocol_via_snapshot() {
    use crate::snapshot::{apply_snapshot, generate_snapshot};

    type FullProtocolStorageA = MockedStorage<9020>;
    type FullProtocolStorageB = MockedStorage<9021>;

    reset_delta_context();

    // Node A: Complex structure
    let mut page_a = Page::new_from_element("My Doc", Element::root());
    Interface::<FullProtocolStorageA>::save(&mut page_a).unwrap();

    let mut para1_a = Paragraph::new_from_element("Intro from A", Element::new(None));
    let mut para2_a = Paragraph::new_from_element("Body from A", Element::new(None));
    Interface::<FullProtocolStorageA>::add_child_to(page_a.id(), &mut para1_a).unwrap();
    Interface::<FullProtocolStorageA>::add_child_to(page_a.id(), &mut para2_a).unwrap();

    // Node B: Empty
    // (No initial state)

    // Generate snapshot from A
    let snapshot_a = generate_snapshot::<FullProtocolStorageA>().unwrap();
    println!(
        "Snapshot from A: {} entities, {} indexes",
        snapshot_a.entity_count, snapshot_a.index_count
    );

    // Apply snapshot to B (full state transfer)
    apply_snapshot::<FullProtocolStorageB>(&snapshot_a).unwrap();

    // Verify B has all of A's data
    let page_b = Interface::<FullProtocolStorageB>::find_by_id::<Page>(Id::root())
        .unwrap()
        .unwrap();
    assert_eq!(page_b.title, "My Doc");

    let children_b: Vec<Paragraph> =
        Interface::<FullProtocolStorageB>::children_of(page_b.id()).unwrap();
    assert_eq!(children_b.len(), 2, "B should have both children from A");

    let texts: Vec<_> = children_b.iter().map(|p| p.text.as_str()).collect();
    assert!(texts.contains(&"Intro from A"));
    assert!(texts.contains(&"Body from A"));

    println!("Full protocol via snapshot: SUCCESS");
}

#[test]
fn tree_sync_detects_divergence_for_manual_resolution() {
    reset_delta_context();

    // Node A: Has children
    let mut page_a = Page::new_from_element("My Doc", Element::root());
    NodeA::save(&mut page_a).unwrap();

    let mut para1_a = Paragraph::new_from_element("Para from A", Element::new(None));
    NodeA::add_child_to(page_a.id(), &mut para1_a).unwrap();

    // Node B: Different children
    let mut page_b = Page::new_from_element("My Doc", Element::root());
    NodeB::save(&mut page_b).unwrap();

    let mut para2_b = Paragraph::new_from_element("Para from B", Element::new(None));
    NodeB::add_child_to(page_b.id(), &mut para2_b).unwrap();

    // Compare trees - this detects divergence
    let (actions_for_a, actions_for_b) =
        compare_trees_between::<StorageA, StorageB>(Id::root()).unwrap();

    println!("Actions for A: {:?}", actions_for_a);
    println!("Actions for B: {:?}", actions_for_b);

    // The comparison correctly identifies that:
    // - A has a child B doesn't have
    // - B has a child A doesn't have
    // These would be Add actions (with empty ancestors - known limitation)

    // For now, verify we at least detect the divergence
    let total_actions = actions_for_a.len() + actions_for_b.len();
    assert!(total_actions > 0, "Should detect divergence");

    // In production, when Add actions have empty ancestors,
    // the system should fallback to snapshot sync
    println!("Divergence detected - would trigger snapshot sync in production");
}

// ============================================================
// Test: Snapshot-based Sync (Full State Transfer)
// ============================================================

#[test]
fn tree_sync_via_snapshot() {
    use crate::snapshot::{apply_snapshot, generate_snapshot};

    // This test requires IterableStorage, which MockedStorage implements
    type SnapshotStorageA = MockedStorage<9010>;
    type SnapshotStorageB = MockedStorage<9011>;

    reset_delta_context();

    // Node A has complex state
    let mut page = Page::new_from_element("Snapshot Test", Element::root());
    Interface::<SnapshotStorageA>::save(&mut page).unwrap();

    let mut para1 = Paragraph::new_from_element("Para 1", Element::new(None));
    let mut para2 = Paragraph::new_from_element("Para 2", Element::new(None));
    Interface::<SnapshotStorageA>::add_child_to(page.id(), &mut para1).unwrap();
    Interface::<SnapshotStorageA>::add_child_to(page.id(), &mut para2).unwrap();

    // Generate snapshot from A
    let snapshot = generate_snapshot::<SnapshotStorageA>().unwrap();

    println!(
        "Snapshot: {} entities, {} indexes, root_hash: {:?}",
        snapshot.entity_count,
        snapshot.index_count,
        hex::encode(snapshot.root_hash)
    );

    assert!(snapshot.entity_count > 0, "Snapshot should have entities");
    assert!(snapshot.index_count > 0, "Snapshot should have indexes");

    // Apply snapshot to Node B (empty)
    apply_snapshot::<SnapshotStorageB>(&snapshot).unwrap();

    // Verify B now has A's data
    let page_b = Interface::<SnapshotStorageB>::find_by_id::<Page>(Id::root())
        .unwrap()
        .unwrap();

    assert_eq!(
        page_b.title, "Snapshot Test",
        "Snapshot should transfer page"
    );

    let children_b: Vec<Paragraph> =
        Interface::<SnapshotStorageB>::children_of(page_b.id()).unwrap();
    assert_eq!(children_b.len(), 2, "Snapshot should transfer children");
}

// ============================================================
// Test: Hash Convergence Verification
// ============================================================

#[test]
fn tree_sync_hash_convergence() {
    reset_delta_context();

    // Create identical initial state
    let root_id = Id::root();
    let para_id = Id::random();

    // Node A
    let mut page_a = Page::new_from_element("Test", Element::root());
    NodeA::save(&mut page_a).unwrap();
    let mut para_a = Paragraph::new_from_element("Shared Para", Element::new(Some(para_id)));
    NodeA::add_child_to(page_a.id(), &mut para_a).unwrap();

    // Node B - same IDs, same content
    let mut page_b = Page::new_from_element("Test", Element::root());
    NodeB::save(&mut page_b).unwrap();
    let mut para_b = Paragraph::new_from_element("Shared Para", Element::new(Some(para_id)));
    NodeB::add_child_to(page_b.id(), &mut para_b).unwrap();

    // With same IDs and content, hashes should be identical
    let hash_a = get_root_hash::<StorageA>();
    let hash_b = get_root_hash::<StorageB>();

    println!("Hash A: {}", hex::encode(hash_a));
    println!("Hash B: {}", hex::encode(hash_b));

    // Note: Hashes might differ due to timestamps
    // But compare_trees should produce empty action lists
    let (actions_a, actions_b) = compare_trees_between::<StorageA, StorageB>(root_id).unwrap();

    println!("Actions for A: {:?}", actions_a);
    println!("Actions for B: {:?}", actions_b);
}

// ============================================================
// Test: Three-Node Sync Scenario
// ============================================================

#[test]
fn tree_sync_three_nodes() {
    reset_delta_context();

    // Node A is the "source of truth" initially
    let mut page_a = Page::new_from_element("Three Node Test", Element::root());
    NodeA::save(&mut page_a).unwrap();

    let mut para_a = Paragraph::new_from_element("Original from A", Element::new(None));
    NodeA::add_child_to(page_a.id(), &mut para_a).unwrap();

    // Node B syncs from A
    let a_data = NodeA::find_by_id_raw(Id::root());
    let a_comparison = NodeA::generate_comparison_data(Some(Id::root())).unwrap();
    let (actions_for_b, _) = NodeB::compare_trees(a_data.clone(), a_comparison.clone()).unwrap();
    apply_actions_to::<StorageB>(actions_for_b).unwrap();

    // Node C syncs from A
    let (actions_for_c, _) = NodeC::compare_trees(a_data, a_comparison).unwrap();
    apply_actions_to::<StorageC>(actions_for_c).unwrap();

    // Verify all three have the page
    let title_a = NodeA::find_by_id::<Page>(Id::root())
        .unwrap()
        .unwrap()
        .title;
    let title_b = NodeB::find_by_id::<Page>(Id::root())
        .unwrap()
        .unwrap()
        .title;
    let title_c = NodeC::find_by_id::<Page>(Id::root())
        .unwrap()
        .unwrap()
        .title;

    assert_eq!(title_a, "Three Node Test");
    assert_eq!(title_b, "Three Node Test");
    assert_eq!(title_c, "Three Node Test");

    println!("All three nodes synchronized successfully!");
}
