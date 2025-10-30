#![allow(unused_results)] // Test code doesn't need to check all return values
//! Delta creation and commit tests
//!
//! Tests for the storage delta lifecycle:
//! - Delta creation from actions
//! - Causal delta commit
//! - DAG head tracking
//! - Action recording

use super::common::{Page, Paragraph};
use crate::address::{Id, Path};
use crate::delta::{
    commit_causal_delta, get_current_heads, push_action, set_current_heads, CausalDelta,
};
use crate::entities::{Data, Element, Metadata};
use crate::interface::Interface;
use crate::store::MockedStorage;

type TestStorage = MockedStorage<6000>;
type TestInterface = Interface<TestStorage>;

#[test]
fn delta_creation_with_no_actions() {
    // Set initial heads
    set_current_heads(vec![[0; 32]]);

    // Commit without any actions
    let root_hash = [1; 32];
    let result = commit_causal_delta(&root_hash);

    // Should return None (no delta created)
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
fn delta_creation_with_single_action() {
    // Reset context
    set_current_heads(vec![[0; 32]]);

    // Create and save a page (generates action)
    let mut page = Page::new_from_element("Test Page", Element::root());
    TestInterface::save(&mut page).unwrap();

    // The save should have pushed an action
    // Now commit the delta
    let root_hash = [1; 32];
    let result = commit_causal_delta(&root_hash);

    assert!(result.is_ok());
    let delta = result.unwrap();

    // Should have a delta
    assert!(delta.is_some());
    let delta = delta.unwrap();

    // Verify delta structure
    assert_eq!(delta.parents, vec![[0; 32]]);
    assert_eq!(delta.actions.len(), 1); // One Add action
    assert!(delta.physical_time() > 0); // HLC contains physical time

    // After commit, heads should be updated to this delta
    let heads = get_current_heads();
    assert_eq!(heads, vec![delta.id]);
}

#[test]
fn delta_creation_with_multiple_actions() {
    set_current_heads(vec![[0; 32]]);

    // Create multiple entities
    let mut page = Page::new_from_element("Page", Element::root());
    TestInterface::save(&mut page).unwrap();

    let mut para1 = Paragraph::new_from_element("Para 1", Element::new(None));
    let mut para2 = Paragraph::new_from_element("Para 2", Element::new(None));

    TestInterface::add_child_to(page.id(), &mut para1).unwrap();
    TestInterface::add_child_to(page.id(), &mut para2).unwrap();

    // Commit delta
    let root_hash = [1; 32];
    let delta = commit_causal_delta(&root_hash).unwrap().unwrap();

    // Should have multiple actions
    assert!(delta.actions.len() >= 2); // At least para1 and para2
}

#[test]
fn delta_id_is_content_addressed() {
    set_current_heads(vec![[0; 32]]);

    let parents = vec![[0; 32]];
    let actions = vec![]; // Empty for simplicity
    let hlc = crate::env::hlc_timestamp();

    // Compute ID twice with same inputs
    let id1 = CausalDelta::compute_id(&parents, &actions, &hlc);
    let id2 = CausalDelta::compute_id(&parents, &actions, &hlc);

    // Should be identical
    assert_eq!(id1, id2);
}

#[test]
fn delta_id_changes_with_parents() {
    let actions = vec![];
    let hlc = crate::env::hlc_timestamp();

    let id1 = CausalDelta::compute_id(&vec![[0; 32]], &actions, &hlc);
    let id2 = CausalDelta::compute_id(&vec![[1; 32]], &actions, &hlc);

    // Should be different
    assert_ne!(id1, id2);
}

#[test]
fn delta_id_deterministic_regardless_of_hlc() {
    let parents = vec![[0; 32]];
    let actions = vec![];

    let hlc1 = crate::env::hlc_timestamp();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let hlc2 = crate::env::hlc_timestamp();

    let id1 = CausalDelta::compute_id(&parents, &actions, &hlc1);
    let id2 = CausalDelta::compute_id(&parents, &actions, &hlc2);

    // Should be the SAME - delta ID is deterministic based on parents+actions only.
    // This ensures nodes executing the same operations produce identical delta IDs.
    assert_eq!(id1, id2);
}

#[test]
fn delta_sequential_commits() {
    use crate::delta::reset_delta_context;
    use crate::env::reset_for_testing;

    set_current_heads(vec![[0; 32]]);

    // First commit
    let mut page1 = Page::new_from_element("Page 1", Element::root());
    TestInterface::save(&mut page1).unwrap();
    let delta1 = commit_causal_delta(&[1; 32]).unwrap().unwrap();

    // Verify first delta
    assert_eq!(delta1.parents, vec![[0; 32]]);
    assert_eq!(get_current_heads(), vec![delta1.id]);

    // Reset environment for second commit
    reset_for_testing();
    reset_delta_context();
    set_current_heads(vec![delta1.id]);

    // Second commit (should have delta1 as parent)
    let mut para = Paragraph::new_from_element("Para", Element::new(None));
    TestInterface::add_child_to(page1.id(), &mut para).unwrap();
    let delta2 = commit_causal_delta(&[2; 32]).unwrap().unwrap();

    // Delta2 should have delta1 as parent
    assert_eq!(delta2.parents, vec![delta1.id]);
    assert_eq!(get_current_heads(), vec![delta2.id]);
}

#[test]
fn delta_concurrent_branch_setup() {
    use crate::delta::reset_delta_context;
    use crate::env::reset_for_testing;

    // Simulate two nodes starting from same parent
    set_current_heads(vec![[0; 32]]);

    // Node 1 creates delta
    let mut page1 = Page::new_from_element("Node 1 Page", Element::root());
    TestInterface::save(&mut page1).unwrap();
    let delta1 = commit_causal_delta(&[1; 32]).unwrap().unwrap();

    // Reset for node 2
    reset_for_testing();
    reset_delta_context();
    set_current_heads(vec![[0; 32]]);

    // Node 2 creates delta from same parent (use root element to avoid orphan)
    type Storage2 = MockedStorage<6001>;
    type Interface2 = Interface<Storage2>;
    let mut page2 = Page::new_from_element("Node 2 Page", Element::root());
    Interface2::save(&mut page2).unwrap();
    let delta2 = commit_causal_delta(&[2; 32]).unwrap().unwrap();

    // Both should have same parent
    assert_eq!(delta1.parents, vec![[0; 32]]);
    assert_eq!(delta2.parents, vec![[0; 32]]);

    // But different IDs (different actions/timestamps)
    assert_ne!(delta1.id, delta2.id);
}

#[test]
fn delta_merge_two_heads() {
    // Setup two concurrent heads
    let head1 = [1; 32];
    let head2 = [2; 32];

    set_current_heads(vec![head1, head2]);

    // Create merge delta
    let mut page = Page::new_from_element("Merge", Element::root());
    TestInterface::save(&mut page).unwrap();
    let merge_delta = commit_causal_delta(&[3; 32]).unwrap().unwrap();

    // Should have both heads as parents
    assert_eq!(merge_delta.parents.len(), 2);
    assert!(merge_delta.parents.contains(&head1));
    assert!(merge_delta.parents.contains(&head2));

    // Heads should now be the merge
    assert_eq!(get_current_heads(), vec![merge_delta.id]);
}

#[test]
fn delta_action_recording() {
    use crate::action::Action;

    set_current_heads(vec![[0; 32]]);

    // Manually push an action
    let test_action = Action::Add {
        id: Id::random(),
        data: vec![1, 2, 3],
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    push_action(test_action.clone());

    // Commit delta
    let delta = commit_causal_delta(&[1; 32]).unwrap().unwrap();

    // Should contain the action
    assert_eq!(delta.actions.len(), 1);
    assert_eq!(delta.actions[0], test_action);
}

#[test]
fn delta_clears_actions_after_commit() {
    use crate::delta::reset_delta_context;
    use crate::env::reset_for_testing;

    set_current_heads(vec![[0; 32]]);

    // Create action
    let mut page = Page::new_from_element("Page", Element::root());
    TestInterface::save(&mut page).unwrap();

    // Commit
    let delta1 = commit_causal_delta(&[1; 32]).unwrap().unwrap();
    assert!(delta1.actions.len() > 0);

    // Reset for second commit
    reset_for_testing();
    reset_delta_context();
    set_current_heads(vec![delta1.id]);

    // Create another action
    let mut para = Paragraph::new_from_element("Para", Element::new(None));
    TestInterface::add_child_to(page.id(), &mut para).unwrap();

    // Commit again
    let delta2 = commit_causal_delta(&[2; 32]).unwrap().unwrap();

    // Should only have new action (context was cleared)
    assert!(delta2.actions.len() > 0);
    assert_ne!(delta1.actions, delta2.actions);
}

#[test]
fn delta_empty_heads_treated_as_genesis() {
    // No heads set (default state)
    set_current_heads(vec![]);

    let mut page = Page::new_from_element("Page", Element::root());
    TestInterface::save(&mut page).unwrap();

    let delta = commit_causal_delta(&[1; 32]).unwrap().unwrap();

    // Should have empty parents (genesis)
    assert_eq!(delta.parents, Vec::<[u8; 32]>::new());
}

#[test]
fn delta_three_way_merge() {
    let head1 = [1; 32];
    let head2 = [2; 32];
    let head3 = [3; 32];

    set_current_heads(vec![head1, head2, head3]);

    let mut page = Page::new_from_element("Three-way merge", Element::root());
    TestInterface::save(&mut page).unwrap();

    let merge_delta = commit_causal_delta(&[4; 32]).unwrap().unwrap();

    // Should have all three as parents
    assert_eq!(merge_delta.parents.len(), 3);
    assert!(merge_delta.parents.contains(&head1));
    assert!(merge_delta.parents.contains(&head2));
    assert!(merge_delta.parents.contains(&head3));
}

#[test]
fn delta_timestamp_is_monotonic() {
    use crate::delta::reset_delta_context;
    use crate::env::reset_for_testing;

    set_current_heads(vec![[0; 32]]);

    // Create first delta
    let mut page1 = Page::new_from_element("Page 1", Element::root());
    TestInterface::save(&mut page1).unwrap();
    let delta1 = commit_causal_delta(&[1; 32]).unwrap().unwrap();

    // Small delay
    std::thread::sleep(std::time::Duration::from_millis(2));

    // Reset for second commit
    reset_for_testing();
    reset_delta_context();
    set_current_heads(vec![delta1.id]);

    // Create second delta
    let mut para = Paragraph::new_from_element("Para", Element::new(None));
    TestInterface::add_child_to(page1.id(), &mut para).unwrap();
    let delta2 = commit_causal_delta(&[2; 32]).unwrap().unwrap();

    // Second delta should have later HLC timestamp
    assert!(delta2.hlc > delta1.hlc);
}

#[test]
fn delta_preserves_action_order() {
    use crate::action::Action;

    set_current_heads(vec![[0; 32]]);

    let id1 = Id::new([1; 32]);
    let id2 = Id::new([2; 32]);
    let id3 = Id::new([3; 32]);

    // Push actions in specific order
    push_action(Action::Add {
        id: id1,
        data: b"first".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    });

    push_action(Action::Add {
        id: id2,
        data: b"second".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    });

    push_action(Action::Add {
        id: id3,
        data: b"third".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    });

    let delta = commit_causal_delta(&[1; 32]).unwrap().unwrap();

    // Actions should be in order
    assert_eq!(delta.actions.len(), 3);
    match &delta.actions[0] {
        Action::Add { id, .. } => assert_eq!(*id, id1),
        _ => panic!("Expected Add action"),
    }
    match &delta.actions[1] {
        Action::Add { id, .. } => assert_eq!(*id, id2),
        _ => panic!("Expected Add action"),
    }
    match &delta.actions[2] {
        Action::Add { id, .. } => assert_eq!(*id, id3),
        _ => panic!("Expected Add action"),
    }
}

#[test]
fn delta_update_action_recorded() {
    use crate::delta::reset_delta_context;
    use crate::env::reset_for_testing;

    set_current_heads(vec![[0; 32]]);

    // Create initial entity
    let mut page = Page::new_from_element("Initial", Element::root());
    TestInterface::save(&mut page).unwrap();
    let delta1 = commit_causal_delta(&[1; 32]).unwrap().unwrap();

    // Reset for second commit
    reset_for_testing();
    reset_delta_context();
    set_current_heads(vec![delta1.id]);

    // Update entity
    page.title = "Updated".to_string();
    page.element_mut().update();
    TestInterface::save(&mut page).unwrap();

    let delta2 = commit_causal_delta(&[2; 32]).unwrap().unwrap();

    // Should have Update action
    assert_eq!(delta2.actions.len(), 1);
    match &delta2.actions[0] {
        crate::action::Action::Update { id, .. } => assert_eq!(*id, page.id()),
        _ => panic!("Expected Update action"),
    }
}

#[test]
fn delta_delete_action_recorded() {
    use crate::action::Action;
    use crate::env::time_now;

    set_current_heads(vec![[0; 32]]);

    let id = Id::random();

    // Push delete action
    push_action(Action::DeleteRef {
        id,
        deleted_at: time_now(),
    });

    let delta = commit_causal_delta(&[1; 32]).unwrap().unwrap();

    // Should have DeleteRef action
    assert_eq!(delta.actions.len(), 1);
    match &delta.actions[0] {
        Action::DeleteRef { id: action_id, .. } => assert_eq!(*action_id, id),
        _ => panic!("Expected DeleteRef action"),
    }
}
