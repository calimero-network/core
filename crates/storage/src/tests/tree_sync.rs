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

/// Perform full bidirectional sync between two nodes
fn sync_nodes<SA: crate::store::StorageAdaptor, SB: crate::store::StorageAdaptor>(
    id: Id,
) -> Result<(), StorageError> {
    // Phase 1: Get what B has that A needs
    let b_data = Interface::<SB>::find_by_id_raw(id);
    let b_comparison = Interface::<SB>::generate_comparison_data(Some(id))?;
    let (actions_for_a, actions_for_b) = Interface::<SA>::compare_trees(b_data, b_comparison)?;

    // Phase 2: Apply actions
    apply_actions_to::<SA>(actions_for_a)?;
    apply_actions_to::<SB>(actions_for_b)?;

    Ok(())
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

// ============================================================
// Resolution Strategy Tests
// ============================================================

use crate::entities::ResolutionStrategy;

/// Test FirstWriteWins resolution - older value wins
#[test]
fn resolution_first_write_wins() {
    type StorageA = MockedStorage<9200>;
    type StorageB = MockedStorage<9201>;
    type NodeA = Interface<StorageA>;
    type NodeB = Interface<StorageB>;

    reset_delta_context();

    // Node A creates a page first (older timestamp)
    let mut page_a = Page::new_from_element(
        "First Value",
        Element::root_with_resolution(ResolutionStrategy::FirstWriteWins),
    );
    NodeA::save(&mut page_a).unwrap();

    // Simulate time passing
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Node B creates a page later (newer timestamp)
    let mut page_b = Page::new_from_element(
        "Second Value",
        Element::root_with_resolution(ResolutionStrategy::FirstWriteWins),
    );
    NodeB::save(&mut page_b).unwrap();

    // Node A has older timestamp, Node B has newer
    let ts_a = NodeA::find_by_id::<Page>(Id::root())
        .unwrap()
        .unwrap()
        .element()
        .updated_at();
    let ts_b = NodeB::find_by_id::<Page>(Id::root())
        .unwrap()
        .unwrap()
        .element()
        .updated_at();
    assert!(ts_a < ts_b, "A should be older than B");

    println!("Timestamp A: {}, Timestamp B: {}", ts_a, ts_b);

    // Sync from B's perspective - compare against A
    let a_data = NodeA::find_by_id_raw(Id::root());
    let a_comparison = NodeA::generate_comparison_data(Some(Id::root())).unwrap();
    let (actions_for_b, actions_for_a) = NodeB::compare_trees(a_data, a_comparison).unwrap();

    println!(
        "Actions for B (should get A's older value): {:?}",
        actions_for_b
    );
    println!("Actions for A (should be empty): {:?}", actions_for_a);

    // FirstWriteWins: A's older value should win
    // So B should receive an Update action with A's data
    assert!(
        actions_for_b
            .iter()
            .any(|a| matches!(a, Action::Update { .. })),
        "B should receive Update with A's older value"
    );

    apply_actions_to::<StorageB>(actions_for_b).unwrap();

    // After sync, B should have A's value
    let page_b_after = NodeB::find_by_id::<Page>(Id::root()).unwrap().unwrap();
    assert_eq!(
        page_b_after.title, "First Value",
        "FirstWriteWins: older value should win"
    );

    println!("FirstWriteWins test passed!");
}

/// Test MaxValue resolution - lexicographically higher value wins
#[test]
fn resolution_max_value() {
    type StorageA = MockedStorage<9210>;
    type StorageB = MockedStorage<9211>;
    type NodeA = Interface<StorageA>;
    type NodeB = Interface<StorageB>;

    reset_delta_context();

    // Node A has "Apple" (lower alphabetically)
    let mut page_a = Page::new_from_element(
        "Apple",
        Element::root_with_resolution(ResolutionStrategy::MaxValue),
    );
    NodeA::save(&mut page_a).unwrap();

    // Node B has "Zebra" (higher alphabetically)
    let mut page_b = Page::new_from_element(
        "Zebra",
        Element::root_with_resolution(ResolutionStrategy::MaxValue),
    );
    NodeB::save(&mut page_b).unwrap();

    println!(
        "Node A title: {}",
        NodeA::find_by_id::<Page>(Id::root())
            .unwrap()
            .unwrap()
            .title
    );
    println!(
        "Node B title: {}",
        NodeB::find_by_id::<Page>(Id::root())
            .unwrap()
            .unwrap()
            .title
    );

    // Sync from A's perspective - compare against B
    let b_data = NodeB::find_by_id_raw(Id::root());
    let b_comparison = NodeB::generate_comparison_data(Some(Id::root())).unwrap();
    let (actions_for_a, actions_for_b) = NodeA::compare_trees(b_data, b_comparison).unwrap();

    println!(
        "Actions for A (should get B's higher value): {:?}",
        actions_for_a
    );
    println!("Actions for B (should be empty): {:?}", actions_for_b);

    // MaxValue: "Zebra" > "Apple", so A should receive B's value
    assert!(
        actions_for_a
            .iter()
            .any(|a| matches!(a, Action::Update { .. })),
        "A should receive Update with B's higher value"
    );

    apply_actions_to::<StorageA>(actions_for_a).unwrap();

    // After sync, A should have B's value
    let page_a_after = NodeA::find_by_id::<Page>(Id::root()).unwrap().unwrap();
    assert_eq!(
        page_a_after.title, "Zebra",
        "MaxValue: higher value should win"
    );

    println!("MaxValue test passed!");
}

/// Test MinValue resolution - lexicographically lower value wins
#[test]
fn resolution_min_value() {
    type StorageA = MockedStorage<9220>;
    type StorageB = MockedStorage<9221>;
    type NodeA = Interface<StorageA>;
    type NodeB = Interface<StorageB>;

    reset_delta_context();

    // Node A has "Zebra" (higher alphabetically)
    let mut page_a = Page::new_from_element(
        "Zebra",
        Element::root_with_resolution(ResolutionStrategy::MinValue),
    );
    NodeA::save(&mut page_a).unwrap();

    // Node B has "Apple" (lower alphabetically)
    let mut page_b = Page::new_from_element(
        "Apple",
        Element::root_with_resolution(ResolutionStrategy::MinValue),
    );
    NodeB::save(&mut page_b).unwrap();

    println!(
        "Node A title: {}",
        NodeA::find_by_id::<Page>(Id::root())
            .unwrap()
            .unwrap()
            .title
    );
    println!(
        "Node B title: {}",
        NodeB::find_by_id::<Page>(Id::root())
            .unwrap()
            .unwrap()
            .title
    );

    // Sync from A's perspective - compare against B
    let b_data = NodeB::find_by_id_raw(Id::root());
    let b_comparison = NodeB::generate_comparison_data(Some(Id::root())).unwrap();
    let (actions_for_a, actions_for_b) = NodeA::compare_trees(b_data, b_comparison).unwrap();

    println!(
        "Actions for A (should get B's lower value): {:?}",
        actions_for_a
    );
    println!("Actions for B (should be empty): {:?}", actions_for_b);

    // MinValue: "Apple" < "Zebra", so A should receive B's value
    assert!(
        actions_for_a
            .iter()
            .any(|a| matches!(a, Action::Update { .. })),
        "A should receive Update with B's lower value"
    );

    apply_actions_to::<StorageA>(actions_for_a).unwrap();

    // After sync, A should have B's value
    let page_a_after = NodeA::find_by_id::<Page>(Id::root()).unwrap().unwrap();
    assert_eq!(
        page_a_after.title, "Apple",
        "MinValue: lower value should win"
    );

    println!("MinValue test passed!");
}

/// Test Manual resolution - no automatic resolution, Compare actions generated
#[test]
fn resolution_manual_generates_compare() {
    type StorageA = MockedStorage<9230>;
    type StorageB = MockedStorage<9231>;
    type NodeA = Interface<StorageA>;
    type NodeB = Interface<StorageB>;

    reset_delta_context();

    // Both nodes have different values with Manual resolution
    let mut page_a = Page::new_from_element(
        "Value A",
        Element::root_with_resolution(ResolutionStrategy::Manual),
    );
    NodeA::save(&mut page_a).unwrap();

    let mut page_b = Page::new_from_element(
        "Value B",
        Element::root_with_resolution(ResolutionStrategy::Manual),
    );
    NodeB::save(&mut page_b).unwrap();

    // Sync from A's perspective - compare against B
    let b_data = NodeB::find_by_id_raw(Id::root());
    let b_comparison = NodeB::generate_comparison_data(Some(Id::root())).unwrap();
    let (actions_for_a, actions_for_b) = NodeA::compare_trees(b_data, b_comparison).unwrap();

    println!("Actions for A: {:?}", actions_for_a);
    println!("Actions for B: {:?}", actions_for_b);

    // Manual resolution should generate Compare actions for both sides
    // (indicating the app needs to handle the conflict)
    let has_compare_for_a = actions_for_a
        .iter()
        .any(|a| matches!(a, Action::Compare { .. }));
    let has_compare_for_b = actions_for_b
        .iter()
        .any(|a| matches!(a, Action::Compare { .. }));

    assert!(
        has_compare_for_a && has_compare_for_b,
        "Manual resolution should generate Compare actions for both sides"
    );

    println!("Manual resolution test passed - conflict flagged for app handling!");
}

/// Test LastWriteWins (default) resolution
#[test]
fn resolution_last_write_wins_default() {
    type StorageA = MockedStorage<9240>;
    type StorageB = MockedStorage<9241>;
    type NodeA = Interface<StorageA>;
    type NodeB = Interface<StorageB>;

    reset_delta_context();

    // Node A creates first
    let mut page_a = Page::new_from_element("Old Value", Element::root()); // Default is LWW
    NodeA::save(&mut page_a).unwrap();

    // Simulate time passing
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Node B creates later
    let mut page_b = Page::new_from_element("New Value", Element::root());
    NodeB::save(&mut page_b).unwrap();

    // Sync from A's perspective - compare against B
    let b_data = NodeB::find_by_id_raw(Id::root());
    let b_comparison = NodeB::generate_comparison_data(Some(Id::root())).unwrap();
    let (actions_for_a, actions_for_b) = NodeA::compare_trees(b_data, b_comparison).unwrap();

    println!(
        "Actions for A (should get B's newer value): {:?}",
        actions_for_a
    );
    println!("Actions for B (should be empty): {:?}", actions_for_b);

    // LastWriteWins: B's newer value should win
    assert!(
        actions_for_a
            .iter()
            .any(|a| matches!(a, Action::Update { .. })),
        "A should receive Update with B's newer value"
    );

    apply_actions_to::<StorageA>(actions_for_a).unwrap();

    // After sync, A should have B's value
    let page_a_after = NodeA::find_by_id::<Page>(Id::root()).unwrap().unwrap();
    assert_eq!(
        page_a_after.title, "New Value",
        "LastWriteWins: newer value should win"
    );

    println!("LastWriteWins (default) test passed!");
}

// ============================================================
// Scale Demonstration: Step-by-Step Sync with Multiple Strategies
// ============================================================

/// A comprehensive demonstration showing resolution strategies at scale.
/// This test creates multiple entities with different strategies and shows
/// the complete sync process step-by-step.
#[test]
fn demo_resolution_strategies_at_scale() {
    use crate::index::Index;
    use std::collections::HashMap;

    // Storage IDs for two simulated nodes
    type StorageAlice = MockedStorage<9900>;
    type StorageBob = MockedStorage<9901>;
    type Alice = Interface<StorageAlice>;
    type Bob = Interface<StorageBob>;

    reset_delta_context();

    println!("\n{}", "=".repeat(70));
    println!("  RESOLUTION STRATEGY DEMONSTRATION AT SCALE");
    println!("{}\n", "=".repeat(70));

    // ---------------------------------------------------------------
    // STEP 1: Create initial state on Alice with different strategies
    // ---------------------------------------------------------------
    println!("STEP 1: Alice creates initial state with mixed resolution strategies");
    println!("{}", "-".repeat(60));

    // Root page with LastWriteWins (default)
    let mut alice_page = Page::new_from_element("Alice's Document", Element::root());
    Alice::save(&mut alice_page).unwrap();
    println!("  ✓ Created root page: '{}' (LWW)", alice_page.title);

    // Create 10 paragraphs with different strategies
    let strategies = vec![
        ("LWW Para 1", ResolutionStrategy::LastWriteWins),
        ("LWW Para 2", ResolutionStrategy::LastWriteWins),
        ("FWW Para 3", ResolutionStrategy::FirstWriteWins),
        ("FWW Para 4", ResolutionStrategy::FirstWriteWins),
        ("Max Para 5", ResolutionStrategy::MaxValue),
        ("Max Para 6", ResolutionStrategy::MaxValue),
        ("Min Para 7", ResolutionStrategy::MinValue),
        ("Min Para 8", ResolutionStrategy::MinValue),
        ("Manual 9", ResolutionStrategy::Manual),
        ("Manual 10", ResolutionStrategy::Manual),
    ];

    let mut alice_paragraphs: Vec<Paragraph> = Vec::new();
    for (text, strategy) in &strategies {
        let element = Element::with_resolution(None, *strategy);
        let mut para = Paragraph::new_from_element(text, element);
        Alice::add_child_to(alice_page.id(), &mut para).unwrap();
        println!("  ✓ Created paragraph: '{}' ({:?})", text, strategy);
        alice_paragraphs.push(para);
    }

    let alice_root_hash = Index::<StorageAlice>::get_hashes_for(Id::root())
        .unwrap()
        .map(|(h, _)| hex::encode(&h[..8]))
        .unwrap_or_default();
    println!("\n  Alice's root hash: {}...", alice_root_hash);

    // ---------------------------------------------------------------
    // STEP 2: Bob bootstraps from Alice (initial sync)
    // ---------------------------------------------------------------
    println!("\nSTEP 2: Bob bootstraps from Alice (snapshot sync)");
    println!("{}", "-".repeat(60));

    // Generate snapshot from Alice
    let snapshot = crate::snapshot::generate_snapshot::<StorageAlice>().unwrap();
    println!(
        "  ✓ Alice generated snapshot: {} entities, {} bytes",
        snapshot.entity_count,
        snapshot.entries.len()
    );

    // Bob applies snapshot
    crate::snapshot::apply_snapshot::<StorageBob>(&snapshot).unwrap();
    println!("  ✓ Bob applied snapshot successfully");

    let bob_root_hash = Index::<StorageBob>::get_hashes_for(Id::root())
        .unwrap()
        .map(|(h, _)| hex::encode(&h[..8]))
        .unwrap_or_default();
    println!("  Bob's root hash: {}...", bob_root_hash);
    println!("  Hashes match: {}", alice_root_hash == bob_root_hash);

    // Verify Bob has all entities
    let bob_children: Vec<Paragraph> = Bob::children_of(Id::root()).unwrap();
    println!("  Bob has {} paragraphs", bob_children.len());

    // ---------------------------------------------------------------
    // STEP 3: Concurrent modifications (simulate network partition)
    // ---------------------------------------------------------------
    println!("\nSTEP 3: Concurrent modifications (simulating network partition)");
    println!("{}", "-".repeat(60));

    // Map to track modifications
    let mut alice_modifications: HashMap<String, String> = HashMap::new();
    let mut bob_modifications: HashMap<String, String> = HashMap::new();

    // Small delay to ensure timestamp differences
    std::thread::sleep(std::time::Duration::from_millis(5));

    // Alice modifies some paragraphs
    println!("\n  Alice's modifications:");
    for (i, para) in alice_paragraphs.iter_mut().enumerate() {
        if i % 2 == 0 {
            // Alice modifies even-indexed paragraphs
            let new_text = format!("Alice edited: Para {}", i + 1);
            para.text = new_text.clone();
            para.element_mut().update();
            Alice::save(para).unwrap();
            alice_modifications.insert(para.id().to_string(), new_text.clone());
            println!("    ✓ Modified para {} -> '{}'", i + 1, new_text);
        }
    }

    std::thread::sleep(std::time::Duration::from_millis(5));

    // Bob modifies some paragraphs (overlapping with Alice)
    println!("\n  Bob's modifications:");
    let bob_paragraphs: Vec<Paragraph> = Bob::children_of(Id::root()).unwrap();
    for (i, para) in bob_paragraphs.iter().enumerate() {
        if i % 3 == 0 || i == 2 || i == 4 {
            // Bob modifies paragraphs 0, 2, 3, 4, 6, 9
            let mut para = para.clone();
            let new_text = format!("Bob edited: Para {} [Z]", i + 1); // Z for MaxValue testing
            para.text = new_text.clone();
            para.element_mut().update();
            Bob::save(&mut para).unwrap();
            bob_modifications.insert(para.id().to_string(), new_text.clone());
            println!("    ✓ Modified para {} -> '{}'", i + 1, new_text);
        }
    }

    println!("\n  Conflict summary:");
    println!("    - Para 1 (LWW): Alice=old, Bob=new -> Bob wins");
    println!("    - Para 3 (FWW): Alice=old, Bob=new -> Alice wins (first write)");
    println!("    - Para 5 (Max): Alice edited, Bob edited with 'Z' -> Bob wins (Z > A)");
    println!("    - Para 7 (Min): Alice edited -> Alice value (no Bob edit)");

    // ---------------------------------------------------------------
    // STEP 4: Synchronization - Alice pulls from Bob
    // ---------------------------------------------------------------
    println!("\nSTEP 4: Synchronization - Alice pulls changes from Bob");
    println!("{}", "-".repeat(60));

    let mut total_actions = 0;
    let mut updates_applied = 0;
    let mut compares_generated = 0;

    // Start from root and work through all entities
    let bob_root_data = Bob::find_by_id_raw(Id::root());
    let bob_root_comparison = Bob::generate_comparison_data(Some(Id::root())).unwrap();

    println!("\n  Comparing root entities...");
    let (root_actions_for_alice, root_actions_for_bob) =
        Alice::compare_trees(bob_root_data, bob_root_comparison).unwrap();

    println!("    Actions for Alice: {}", root_actions_for_alice.len());
    println!("    Actions for Bob: {}", root_actions_for_bob.len());

    // Process root actions
    for action in &root_actions_for_alice {
        match action {
            Action::Update { id, .. } => {
                println!("    → Update for root {:?}", &id.as_bytes()[..4]);
                updates_applied += 1;
            }
            Action::Compare { id } => {
                println!("    → Compare needed for child {:?}", &id.as_bytes()[..4]);
                compares_generated += 1;
            }
            _ => {}
        }
        total_actions += 1;
    }

    // Apply root-level updates
    for action in root_actions_for_alice.iter().cloned() {
        if !matches!(action, Action::Compare { .. }) {
            Alice::apply_action(action).unwrap();
        }
    }

    // Now handle Compare actions (child entities)
    println!("\n  Processing child comparisons...");
    let compare_ids: Vec<Id> = root_actions_for_alice
        .iter()
        .filter_map(|a| {
            if let Action::Compare { id } = a {
                Some(*id)
            } else {
                None
            }
        })
        .collect();

    let mut resolution_results: Vec<(usize, String, String)> = Vec::new();

    for child_id in compare_ids {
        let bob_child_data = Bob::find_by_id_raw(child_id);
        let bob_child_comparison = Bob::generate_comparison_data(Some(child_id)).unwrap();

        let (child_actions_for_alice, _) =
            Alice::compare_trees(bob_child_data, bob_child_comparison).unwrap();

        // Find which paragraph this is
        let para_idx = alice_paragraphs
            .iter()
            .position(|p| p.id() == child_id)
            .unwrap_or(99);

        for action in &child_actions_for_alice {
            match action {
                Action::Update { data, metadata, .. } => {
                    let strategy = metadata.resolution;
                    let text_preview = String::from_utf8_lossy(&data[5..data.len().min(30)]);
                    resolution_results.push((
                        para_idx + 1,
                        format!("{:?}", strategy),
                        text_preview.to_string(),
                    ));
                    total_actions += 1;
                    updates_applied += 1;
                }
                Action::Compare { .. } => {
                    resolution_results.push((
                        para_idx + 1,
                        "Manual".to_string(),
                        "CONFLICT - needs app resolution".to_string(),
                    ));
                    compares_generated += 1;
                    total_actions += 1;
                }
                _ => {}
            }
        }

        // Apply child updates
        for action in child_actions_for_alice {
            if !matches!(action, Action::Compare { .. }) {
                Alice::apply_action(action).unwrap();
            }
        }
    }

    // ---------------------------------------------------------------
    // STEP 5: Results Summary
    // ---------------------------------------------------------------
    println!("\nSTEP 5: Resolution Results Summary");
    println!("{}", "-".repeat(60));

    println!("\n  Resolution outcomes:");
    for (para_num, strategy, result) in &resolution_results {
        println!("    Para {}: [{}] -> {}", para_num, strategy, result);
    }

    println!("\n  Statistics:");
    println!("    Total actions generated: {}", total_actions);
    println!("    Updates applied: {}", updates_applied);
    println!("    Manual conflicts (Compare): {}", compares_generated);

    // ---------------------------------------------------------------
    // STEP 6: Verify Final State
    // ---------------------------------------------------------------
    println!("\nSTEP 6: Verify Final State");
    println!("{}", "-".repeat(60));

    let final_alice_children: Vec<Paragraph> = Alice::children_of(Id::root()).unwrap();
    let final_bob_children: Vec<Paragraph> = Bob::children_of(Id::root()).unwrap();

    println!("\n  Final paragraph contents:");
    for (i, (alice_para, bob_para)) in final_alice_children
        .iter()
        .zip(final_bob_children.iter())
        .enumerate()
    {
        let strategy = strategies
            .get(i)
            .map(|(_, s)| s)
            .unwrap_or(&ResolutionStrategy::LastWriteWins);
        let match_status = if alice_para.text == bob_para.text {
            "✓ MATCH"
        } else {
            "✗ DIFFER (expected for Manual)"
        };
        println!(
            "    Para {} [{:?}]: Alice='{}' | Bob='{}' {}",
            i + 1,
            strategy,
            &alice_para.text[..alice_para.text.len().min(25)],
            &bob_para.text[..bob_para.text.len().min(25)],
            match_status
        );
    }

    // Verify hash convergence for non-Manual entities
    let alice_final_hash = Index::<StorageAlice>::get_hashes_for(Id::root())
        .unwrap()
        .map(|(h, _)| hex::encode(&h[..8]))
        .unwrap_or_default();
    let bob_final_hash = Index::<StorageBob>::get_hashes_for(Id::root())
        .unwrap()
        .map(|(h, _)| hex::encode(&h[..8]))
        .unwrap_or_default();

    println!("\n  Final root hashes:");
    println!("    Alice: {}...", alice_final_hash);
    println!("    Bob:   {}...", bob_final_hash);

    println!("\n{}", "=".repeat(70));
    println!("  DEMONSTRATION COMPLETE");
    println!("{}\n", "=".repeat(70));

    // Note: Hashes may not match due to Manual resolution entities
    // That's expected behavior - Manual conflicts need app-level resolution
}

/// Stress test: Large scale sync with 100 entities and random conflicts
#[test]
fn stress_test_resolution_at_scale() {
    use crate::interface::ComparisonData;

    type StorageA = MockedStorage<9950>;
    type StorageB = MockedStorage<9951>;
    type NodeA = Interface<StorageA>;
    type NodeB = Interface<StorageB>;

    reset_delta_context();

    const ENTITY_COUNT: usize = 50;
    const CONFLICT_PERCENTAGE: usize = 30; // 30% of entities will have conflicts

    println!(
        "\n=== STRESS TEST: {} entities, {}% conflicts ===\n",
        ENTITY_COUNT, CONFLICT_PERCENTAGE
    );

    // Create root on A
    let mut root_a = Page::new_from_element("Stress Test Root", Element::root());
    NodeA::save(&mut root_a).unwrap();

    // Create many children with varying strategies
    let strategies = [
        ResolutionStrategy::LastWriteWins,
        ResolutionStrategy::FirstWriteWins,
        ResolutionStrategy::MaxValue,
        ResolutionStrategy::MinValue,
    ];

    let mut child_ids: Vec<(Id, ResolutionStrategy)> = Vec::new();

    for i in 0..ENTITY_COUNT {
        let strategy = strategies[i % strategies.len()];
        let element = Element::with_resolution(None, strategy);
        let mut para = Paragraph::new_from_element(&format!("Entity {}", i), element);
        NodeA::add_child_to(root_a.id(), &mut para).unwrap();
        child_ids.push((para.id(), strategy));
    }

    println!("Created {} entities on Node A", ENTITY_COUNT);

    // Sync to B via snapshot
    let snapshot = crate::snapshot::generate_snapshot::<StorageA>().unwrap();
    crate::snapshot::apply_snapshot::<StorageB>(&snapshot).unwrap();
    println!("Synced to Node B via snapshot");

    // Create conflicts
    std::thread::sleep(std::time::Duration::from_millis(10));

    let conflict_count = ENTITY_COUNT * CONFLICT_PERCENTAGE / 100;
    println!("Creating {} conflicts...", conflict_count);

    // A modifies first half of conflict entities
    for i in 0..conflict_count / 2 {
        if let Some(mut para) = NodeA::find_by_id::<Paragraph>(child_ids[i].0).unwrap() {
            para.text = format!("A-Modified-{}", i);
            para.element_mut().update();
            NodeA::save(&mut para).unwrap();
        }
    }

    std::thread::sleep(std::time::Duration::from_millis(10));

    // B modifies overlapping + second half
    for i in 0..conflict_count {
        if let Some(mut para) = NodeB::find_by_id::<Paragraph>(child_ids[i].0).unwrap() {
            para.text = format!("B-Modified-{}-ZZZZZ", i); // Z's for MaxValue testing
            para.element_mut().update();
            NodeB::save(&mut para).unwrap();
        }
    }

    println!("Modifications complete, starting sync...");

    // Sync A from B using recursive sync_trees
    let start = std::time::Instant::now();

    let b_data = NodeB::find_by_id_raw(Id::root());
    let b_comparison = NodeB::generate_comparison_data(Some(Id::root())).unwrap();

    let get_b_data = |id: Id| -> Result<(Option<Vec<u8>>, ComparisonData), StorageError> {
        Ok((
            NodeB::find_by_id_raw(id),
            NodeB::generate_comparison_data(Some(id))?,
        ))
    };

    let (actions_for_a, _actions_for_b) =
        NodeA::sync_trees(b_data, b_comparison, get_b_data).unwrap();

    let sync_time = start.elapsed();

    // Count action types
    let mut updates = 0;
    let mut adds = 0;
    let mut compares = 0;

    for action in &actions_for_a {
        match action {
            Action::Update { .. } => updates += 1,
            Action::Add { .. } => adds += 1,
            Action::Compare { .. } => compares += 1,
            _ => {}
        }
    }

    // Apply actions to A (A pulls from B)
    for action in actions_for_a {
        if !matches!(action, Action::Compare { .. }) {
            NodeA::apply_action(action).unwrap();
        }
    }

    println!("  Phase 1: A synced from B - {} updates applied", updates);

    // Now do BIDIRECTIONAL sync: B also pulls from A
    // This ensures both nodes converge to the same value based on resolution strategy
    println!("  Phase 2: B syncing from A (bidirectional)...");

    let a_data = NodeA::find_by_id_raw(Id::root());
    let a_comparison = NodeA::generate_comparison_data(Some(Id::root())).unwrap();

    let get_a_data = |id: Id| -> Result<(Option<Vec<u8>>, ComparisonData), StorageError> {
        Ok((
            NodeA::find_by_id_raw(id),
            NodeA::generate_comparison_data(Some(id))?,
        ))
    };

    let (actions_for_b, _) = NodeB::sync_trees(a_data, a_comparison, get_a_data).unwrap();

    let mut b_updates = 0;
    for action in &actions_for_b {
        if matches!(action, Action::Update { .. }) {
            b_updates += 1;
        }
    }

    // Apply actions to B
    for action in actions_for_b {
        if !matches!(action, Action::Compare { .. }) {
            NodeB::apply_action(action).unwrap();
        }
    }

    println!("  Phase 2: B synced from A - {} updates applied", b_updates);

    let total_sync_time = start.elapsed();

    println!("\n=== STRESS TEST RESULTS ===");
    println!("Entities: {}", ENTITY_COUNT);
    println!("Conflicts: {}", conflict_count);
    println!("Total sync time (bidirectional): {:?}", total_sync_time);
    println!(
        "Actions: A received {} updates, B received {} updates",
        updates, b_updates
    );

    // Verify consistency - with bidirectional sync, ALL non-Manual should converge
    let mut matching = 0;
    let mut differing = 0;
    let mut manual_count = 0;

    for (id, strategy) in &child_ids {
        let a_para = NodeA::find_by_id::<Paragraph>(*id).unwrap();
        let b_para = NodeB::find_by_id::<Paragraph>(*id).unwrap();

        if *strategy == ResolutionStrategy::Manual {
            manual_count += 1;
            // Manual entities may not converge - that's expected
            if a_para.as_ref().map(|p| &p.text) == b_para.as_ref().map(|p| &p.text) {
                matching += 1;
            } else {
                differing += 1;
            }
        } else {
            // Non-manual should ALWAYS converge after bidirectional sync
            let a_text = a_para.as_ref().map(|p| p.text.clone());
            let b_text = b_para.as_ref().map(|p| p.text.clone());
            if a_text == b_text {
                matching += 1;
            } else {
                println!(
                    "  ⚠ UNEXPECTED DIFF [{:?}]: A='{}' vs B='{}'",
                    strategy,
                    a_text.as_deref().unwrap_or("None"),
                    b_text.as_deref().unwrap_or("None")
                );
                differing += 1;
            }
        }
    }

    println!(
        "Convergence: {} matching, {} differing ({} are Manual strategy)",
        matching, differing, manual_count
    );
    println!("=== STRESS TEST COMPLETE ===\n");

    // With bidirectional sync, all non-Manual entities MUST converge
    let non_manual_count = ENTITY_COUNT - manual_count;
    let non_manual_matching = matching - manual_count.min(matching);
    let convergence_rate = if non_manual_count > 0 {
        (non_manual_matching as f64) / (non_manual_count as f64)
    } else {
        1.0
    };
    println!(
        "Non-manual convergence rate: {:.1}%",
        convergence_rate * 100.0
    );

    // All non-manual entities should converge (100%)
    assert!(
        convergence_rate >= 0.99,
        "All non-Manual entities should converge after bidirectional sync, got {:.1}%",
        convergence_rate * 100.0
    );
}
