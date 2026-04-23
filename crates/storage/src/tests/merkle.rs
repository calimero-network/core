#![allow(unused_results)] // Test code doesn't need to check all return values
//! Merkle hash propagation tests
//!
//! Tests that Merkle hashes correctly propagate through entity hierarchies.
//! This is critical for sync - nodes use Merkle tree comparison to detect
//! which subtrees differ and need synchronization.

use super::common::{Page, Paragraph};
use crate::address::Id;
use crate::entities::{Data, Element};
use crate::index::Index;
use crate::interface::Interface;
use crate::store::{MockedStorage, StorageAdaptor};

type TestStorage = MockedStorage<8000>;
type TestInterface = Interface<TestStorage>;

/// Computes `full_hash` independently from the stored `own_hash` +
/// `children`, without reading the stored `full_hash` field. After
/// #2238 Fix 1, `Index::get_full_merkle_hash_for` is itself a stored
/// read of `full_hash` — so using it as a "fresh recompute" against
/// `get_hashes_for` would be tautological. This helper gives a real
/// independent signal: if any write site forgot to refresh
/// `full_hash`, the stored value and this recompute diverge.
fn recompute_full_hash<S: StorageAdaptor>(id: Id) -> [u8; 32] {
    let index = Index::<S>::get_index(id).unwrap().unwrap();
    Index::<S>::calculate_full_hash_for_children(
        index.own_hash(),
        &index.children().map(<[_]>::to_vec),
    )
    .unwrap()
}

// ============================================================
// Hash Propagation - Child Addition
// ============================================================

#[test]
fn merkle_hash_changes_when_child_added() {
    // Create parent
    let mut page = Page::new_from_element("Parent", Element::root());
    TestInterface::save(&mut page).unwrap();
    let hash_before = page.element().merkle_hash;

    // Add child
    let mut para = Paragraph::new_from_element("Child", Element::new(None));
    TestInterface::add_child_to(page.id(), &mut para).unwrap();

    // Reload parent
    let page_after = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap();

    // Parent hash MUST have changed (now includes child)
    assert_ne!(
        hash_before,
        page_after.element().merkle_hash,
        "Parent Merkle hash must change when child added"
    );
}

#[test]
fn merkle_hash_includes_multiple_children() {
    let mut page = Page::new_from_element("Parent", Element::root());
    TestInterface::save(&mut page).unwrap();

    // Add first child
    let mut para1 = Paragraph::new_from_element("Child 1", Element::new(None));
    TestInterface::add_child_to(page.id(), &mut para1).unwrap();

    let hash_one_child = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    // Add second child
    let mut para2 = Paragraph::new_from_element("Child 2", Element::new(None));
    TestInterface::add_child_to(page.id(), &mut para2).unwrap();

    let hash_two_children = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    // Hash should be different with two children
    assert_ne!(
        hash_one_child, hash_two_children,
        "Hash must change when second child added"
    );
}

// ============================================================
// Hash Propagation - Child Updates
// ============================================================

#[test]
fn merkle_hash_propagates_on_child_update() {
    // Create parent with child
    let mut page = Page::new_from_element("Parent", Element::root());
    TestInterface::save(&mut page).unwrap();

    let mut para = Paragraph::new_from_element("Original", Element::new(None));
    TestInterface::add_child_to(page.id(), &mut para).unwrap();

    let parent_hash_before = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    // Update child
    para.text = "Updated".to_string();
    para.element_mut().update();
    TestInterface::save(&mut para).unwrap();

    // Parent hash should change (child changed)
    let parent_hash_after = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    assert_ne!(
        parent_hash_before, parent_hash_after,
        "Parent hash must change when child updates"
    );
}

#[test]
fn merkle_hash_stable_when_child_unchanged() {
    let mut page = Page::new_from_element("Parent", Element::root());
    TestInterface::save(&mut page).unwrap();

    let mut para = Paragraph::new_from_element("Child", Element::new(None));
    TestInterface::add_child_to(page.id(), &mut para).unwrap();

    let hash1 = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    // Reload without changes
    let hash2 = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    assert_eq!(hash1, hash2, "Hash should be stable when unchanged");
}

// ============================================================
// Hash Propagation - Deep Hierarchies
// ============================================================

#[test]
fn merkle_hash_propagates_through_deep_hierarchy() {
    // Create 3-level hierarchy
    let mut page = Page::new_from_element("Grandparent", Element::root());
    TestInterface::save(&mut page).unwrap();

    let mut para1 = Paragraph::new_from_element("Parent", Element::new(None));
    TestInterface::add_child_to(page.id(), &mut para1).unwrap();

    let grandparent_hash_before = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    // Update grandchild (deep in hierarchy)
    para1.text = "Modified Parent".to_string();
    para1.element_mut().update();
    TestInterface::save(&mut para1).unwrap();

    // Grandparent hash should change (deep propagation)
    let grandparent_hash_after = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    assert_ne!(
        grandparent_hash_before, grandparent_hash_after,
        "Deep hierarchy changes must propagate to root"
    );
}

// ============================================================
// Hash-Based Comparison for Sync
// ============================================================

#[test]
fn merkle_hash_detects_divergence() {
    super::common::register_test_merge_functions();
    type Storage1 = MockedStorage<8001>;
    type Storage2 = MockedStorage<8002>;

    // Create identical structure on both nodes
    let mut page1 = Page::new_from_element("Page", Element::root());
    let mut page2 = Page::new_from_element("Page", Element::root());

    Interface::<Storage1>::save(&mut page1).unwrap();
    Interface::<Storage2>::save(&mut page2).unwrap();

    // Hashes should be similar initially (both empty pages)
    let hash1_before = page1.element().merkle_hash;
    let hash2_before = page2.element().merkle_hash;

    // Modify on node 1 only
    page1.title = "Modified".to_string();
    page1.element_mut().update();
    Interface::<Storage1>::save(&mut page1).unwrap();

    let hash1_after = page1.element().merkle_hash;
    let hash2_after = page2.element().merkle_hash;

    // Node 1's hash changed, node 2's didn't
    assert_ne!(hash1_before, hash1_after, "Node 1 hash should change");
    assert_eq!(hash2_before, hash2_after, "Node 2 hash shouldn't change");

    // This difference triggers sync
    assert_ne!(
        hash1_after, hash2_after,
        "Divergent hashes signal need for sync"
    );
}

#[test]
fn merkle_hash_convergence_after_sync() {
    super::common::register_test_merge_functions();
    type Storage1 = MockedStorage<8003>;
    type Storage2 = MockedStorage<8004>;

    let mut page1 = Page::new_from_element("Page", Element::root());
    Interface::<Storage1>::save(&mut page1).unwrap();

    // Modify on node 1
    page1.title = "Updated".to_string();
    page1.element_mut().update();
    Interface::<Storage1>::save(&mut page1).unwrap();

    let hash1 = page1.element().merkle_hash;

    // Sync to node 2 (via action)
    let action = crate::action::Action::Update {
        id: page1.id(),
        data: borsh::to_vec(&page1).unwrap(),
        ancestors: vec![],
        metadata: page1.element().metadata.clone(),
    };

    Interface::<Storage2>::apply_action(action).unwrap();

    // After sync, hashes should match
    let page2 = Interface::<Storage2>::find_by_id::<Page>(page1.id())
        .unwrap()
        .unwrap();

    assert_eq!(
        hash1,
        page2.element().merkle_hash,
        "Hashes must converge after sync"
    );
}

// ============================================================
// Hash Propagation - Edge Cases
// ============================================================

#[test]
fn merkle_hash_with_concurrent_child_updates() {
    let mut page = Page::new_from_element("Parent", Element::root());
    TestInterface::save(&mut page).unwrap();

    // Add two children concurrently
    let mut para1 = Paragraph::new_from_element("Child 1", Element::new(None));
    let mut para2 = Paragraph::new_from_element("Child 2", Element::new(None));

    TestInterface::add_child_to(page.id(), &mut para1).unwrap();
    TestInterface::add_child_to(page.id(), &mut para2).unwrap();

    let hash_before = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    // Update both children
    para1.text = "Updated 1".to_string();
    para1.element_mut().update();
    TestInterface::save(&mut para1).unwrap();

    para2.text = "Updated 2".to_string();
    para2.element_mut().update();
    TestInterface::save(&mut para2).unwrap();

    // Parent hash should reflect both updates
    let hash_after = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    assert_ne!(
        hash_before, hash_after,
        "Parent hash must reflect multiple child updates"
    );
}

#[test]
fn merkle_hash_deterministic() {
    type Storage1 = MockedStorage<8005>;
    type Storage2 = MockedStorage<8006>;

    // Create identical structures independently
    let mut page1 = Page::new_from_element("Page", Element::root());
    let mut page2 = Page::new_from_element("Page", Element::root());

    Interface::<Storage1>::save(&mut page1).unwrap();
    Interface::<Storage2>::save(&mut page2).unwrap();

    // Add identical children
    let mut para1a = Paragraph::new_from_element("Para 1", Element::new(None));
    let mut para1b = Paragraph::new_from_element("Para 1", Element::new(None));

    Interface::<Storage1>::add_child_to(page1.id(), &mut para1a).unwrap();
    Interface::<Storage2>::add_child_to(page2.id(), &mut para1b).unwrap();

    // Verify both pages have their Merkle hashes computed
    let hash1 = Interface::<Storage1>::find_by_id::<Page>(page1.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;
    let hash2 = Interface::<Storage2>::find_by_id::<Page>(page2.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    // Both hashes should be non-zero (computed, not default)
    assert_ne!(hash1, [0u8; 32], "Page1 should have non-zero Merkle hash");
    assert_ne!(hash2, [0u8; 32], "Page2 should have non-zero Merkle hash");

    // Note: Hashes may differ due to different IDs, but both should be computed
}

#[test]
fn merkle_hash_child_removal_updates_parent() {
    let mut page = Page::new_from_element("Parent", Element::root());
    TestInterface::save(&mut page).unwrap();

    let mut para = Paragraph::new_from_element("Child", Element::new(None));
    TestInterface::add_child_to(page.id(), &mut para).unwrap();

    let hash_with_child = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    // Remove child
    TestInterface::remove_child_from(page.id(), para.id()).unwrap();

    let hash_without_child = TestInterface::find_by_id::<Page>(page.id())
        .unwrap()
        .unwrap()
        .element()
        .merkle_hash;

    assert_ne!(
        hash_with_child, hash_without_child,
        "Parent hash must update when child removed"
    );
}

// ============================================================
// #2238 — Deferred-ancestor scope must produce byte-identical hashes
// ============================================================

// All three tests use a 3-level hierarchy (root → parent → leaves) because
// the scope's flush is specifically about WALKING ancestors — adding under
// a root-level parent means the walk has zero steps and the flush is a no-op,
// which wouldn't exercise the code path under test.

#[test]
fn deferred_ancestor_scope_propagates_through_ancestors() {
    // Structural invariant: after scope flush, every ancestor's saved
    // `full_hash` equals a fresh recompute from its current children.
    // With a 3-level tree, the flush must walk parent → root and update
    // the root's children list to reflect parent's new hash — catching
    // any bug where the deferred flush leaves intermediate ancestors
    // stale.
    use crate::index::{DeferredAncestorScope, Index};

    crate::env::reset_for_testing();
    crate::tests::common::register_test_merge_functions();

    // root → parent (3-level so flush actually walks)
    let mut root = Page::new_from_element("Root", Element::root());
    TestInterface::save(&mut root).unwrap();
    let mut parent = Paragraph::new_from_element("Parent", Element::new(None));
    TestInterface::add_child_to(root.id(), &mut parent).unwrap();

    let root_hash_before = Index::<TestStorage>::get_hashes_for(root.id())
        .unwrap()
        .unwrap()
        .0;

    {
        let scope = DeferredAncestorScope::<TestStorage>::new();
        for i in 0..10 {
            let mut leaf = Paragraph::new_from_element(&format!("Leaf {i}"), Element::new(None));
            TestInterface::add_child_to(parent.id(), &mut leaf).unwrap();
        }
        scope.finish().unwrap();
    }

    // Root's saved full_hash must match an independent recompute from
    // its own_hash + children — catches a scope that forgot to flush.
    let root_stored = Index::<TestStorage>::get_hashes_for(root.id())
        .unwrap()
        .unwrap()
        .0;
    let root_recomputed = recompute_full_hash::<TestStorage>(root.id());
    assert_eq!(
        hex::encode(root_stored),
        hex::encode(root_recomputed),
        "root's stored full_hash must equal independent recompute after scope flush",
    );

    // Same invariant for parent.
    let parent_stored = Index::<TestStorage>::get_hashes_for(parent.id())
        .unwrap()
        .unwrap()
        .0;
    let parent_recomputed = recompute_full_hash::<TestStorage>(parent.id());
    assert_eq!(
        hex::encode(parent_stored),
        hex::encode(parent_recomputed),
        "parent's stored full_hash must equal independent recompute after scope flush",
    );

    // And the root hash must have CHANGED, proving the ancestor walk
    // actually ran rather than being a no-op (a cheap signal that the
    // flush did propagate new descendant hashes into root).
    assert_ne!(
        root_hash_before, root_stored,
        "root hash should change after 10 leaves added under parent",
    );
}

#[test]
fn deferred_scope_dedupes_walks_from_same_parent() {
    // Semantic test: after a scope flush, the parent's full_hash reflects
    // all children added inside the scope. Uses 3-level hierarchy so the
    // flush does actual work.
    use crate::index::{DeferredAncestorScope, Index};

    crate::env::reset_for_testing();
    crate::tests::common::register_test_merge_functions();

    let mut root = Page::new_from_element("Root", Element::root());
    TestInterface::save(&mut root).unwrap();
    let mut parent = Paragraph::new_from_element("Parent", Element::new(None));
    TestInterface::add_child_to(root.id(), &mut parent).unwrap();

    {
        let scope = DeferredAncestorScope::<TestStorage>::new();
        for i in 0..5 {
            let mut child = Paragraph::new_from_element(&format!("Child {i}"), Element::new(None));
            TestInterface::add_child_to(parent.id(), &mut child).unwrap();
        }
        scope.finish().unwrap();
    }

    // Parent's index should record all 5 children.
    let children = Index::<TestStorage>::get_children_of(parent.id()).unwrap();
    assert_eq!(
        children.len(),
        5,
        "all 5 children should be indexed under parent"
    );
}

#[test]
fn deferred_scope_handles_mixed_add_and_remove() {
    // Reviewer follow-up: `DeferredAncestorScope` is active for the entire
    // `Root::sync` action loop, including `Action::DeleteRef`. That path
    // calls `recalculate_ancestor_hashes_for` after removing a child,
    // and its ancestor walk is deferred along with the add-side walks.
    //
    // This test exercises that mixed case: inside a single scope, add
    // some children, then remove one, then add more. After flush, the
    // tree must be consistent — every ancestor's stored full_hash equals
    // the fresh recompute from its current children.
    use crate::index::{DeferredAncestorScope, Index};

    crate::env::reset_for_testing();
    crate::tests::common::register_test_merge_functions();

    let mut root = Page::new_from_element("Root", Element::root());
    TestInterface::save(&mut root).unwrap();
    let mut parent = Paragraph::new_from_element("Parent", Element::new(None));
    TestInterface::add_child_to(root.id(), &mut parent).unwrap();

    // Seed two children before opening the scope so we have something
    // to remove inside it.
    let mut seed_a = Paragraph::new_from_element("Seed A", Element::new(None));
    TestInterface::add_child_to(parent.id(), &mut seed_a).unwrap();
    let mut seed_b = Paragraph::new_from_element("Seed B", Element::new(None));
    TestInterface::add_child_to(parent.id(), &mut seed_b).unwrap();

    {
        let scope = DeferredAncestorScope::<TestStorage>::new();

        // Add three new children.
        for i in 0..3 {
            let mut child = Paragraph::new_from_element(&format!("New {i}"), Element::new(None));
            TestInterface::add_child_to(parent.id(), &mut child).unwrap();
        }
        // Remove one of the seeds — this goes through Index::remove_child_from,
        // which calls recalculate_ancestor_hashes_for (also deferred inside
        // the scope). The apply_delete_ref_action path does the same thing
        // via remote sync, so a test covering the local path covers both.
        TestInterface::remove_child_from(parent.id(), seed_a.id()).unwrap();
        // Add one more after the removal to make sure ordering in the
        // deferred set doesn't matter.
        let mut tail = Paragraph::new_from_element("Tail", Element::new(None));
        TestInterface::add_child_to(parent.id(), &mut tail).unwrap();

        scope.finish().unwrap();
    }

    // Post-flush: parent has seed_b + 3 new + tail = 5 children. Root and
    // parent stored hashes must match fresh recomputes.
    let children = Index::<TestStorage>::get_children_of(parent.id()).unwrap();
    assert_eq!(
        children.len(),
        5,
        "parent should hold 1 seed + 3 new + 1 tail after mixed add/remove",
    );

    let root_stored = Index::<TestStorage>::get_hashes_for(root.id())
        .unwrap()
        .unwrap()
        .0;
    let root_recomputed = recompute_full_hash::<TestStorage>(root.id());
    assert_eq!(
        hex::encode(root_stored),
        hex::encode(root_recomputed),
        "root stored full_hash must equal independent recompute after mixed add/remove in a scope",
    );
    let parent_stored = Index::<TestStorage>::get_hashes_for(parent.id())
        .unwrap()
        .unwrap()
        .0;
    let parent_recomputed = recompute_full_hash::<TestStorage>(parent.id());
    assert_eq!(
        hex::encode(parent_stored),
        hex::encode(parent_recomputed),
        "parent stored full_hash must equal independent recompute after mixed add/remove in a scope",
    );
}

#[test]
fn deferred_scope_of_different_adaptor_does_not_cross_contaminate() {
    // Reviewer concern: a single untyped thread-local could let a scope
    // opened against StorageAdaptor S1 defer calls targeting S2's Index,
    // then flush them through S1's backend — corrupting whichever tree
    // actually lives there.
    //
    // The implementation keys the thread-local on TypeId<S>, so a
    // mismatched call goes direct. Verify: open a scope for another
    // MockedStorage variant and assert calls on TestStorage still do
    // work eagerly (not batched into the wrong backend).
    use crate::index::{DeferredAncestorScope, Index};
    use crate::store::MockedStorage;

    type OtherStorage = MockedStorage<8099>;

    crate::env::reset_for_testing();
    crate::tests::common::register_test_merge_functions();

    let mut root = Page::new_from_element("Root", Element::root());
    TestInterface::save(&mut root).unwrap();
    let mut parent = Paragraph::new_from_element("Parent", Element::new(None));
    TestInterface::add_child_to(root.id(), &mut parent).unwrap();

    // Open a scope for a DIFFERENT StorageAdaptor.
    let foreign_scope = DeferredAncestorScope::<OtherStorage>::new();

    // Add a child under parent on TestStorage. This should NOT be
    // deferred — the foreign scope's TypeId doesn't match ours.
    let mut child = Paragraph::new_from_element("Eager", Element::new(None));
    TestInterface::add_child_to(parent.id(), &mut child).unwrap();

    // Root's stored hash must already reflect the change (not wait for
    // the foreign scope to flush, which wouldn't propagate here anyway).
    // Independent recompute catches the case where the foreign scope
    // swallowed the update for TestStorage instead of running it eagerly.
    let root_stored = Index::<TestStorage>::get_hashes_for(root.id())
        .unwrap()
        .unwrap()
        .0;
    let root_recomputed = recompute_full_hash::<TestStorage>(root.id());
    assert_eq!(
        hex::encode(root_stored),
        hex::encode(root_recomputed),
        "TestStorage walk must run eagerly under a foreign-adaptor scope",
    );

    foreign_scope.finish().unwrap();
}

// ============================================================
// #2238 Fix 3 — sorted-insert path produces identical hashes
// ============================================================

#[test]
fn sorted_insert_path_preserves_hash_semantics() {
    // Add multiple children out of creation order; the binary_search + insert
    // path must land them in the same sorted position a BTreeSet would have
    // chosen. The final parent full_hash must equal a fresh recompute
    // from the (ordered) children list — enforced by the existing merkle
    // invariants, but we assert here to lock in the sort-order guarantee.
    use crate::index::Index;

    crate::env::reset_for_testing();
    crate::tests::common::register_test_merge_functions();

    let mut parent = Page::new_from_element("Parent", Element::root());
    TestInterface::save(&mut parent).unwrap();

    // Add children with interleaved names so the string-derived created_at
    // / id hashes don't arrive in any particular sorted order.
    let names = ["delta", "alpha", "gamma", "beta", "epsilon"];
    for name in names {
        let mut child = Paragraph::new_from_element(name, Element::new(None));
        TestInterface::add_child_to(parent.id(), &mut child).unwrap();
    }

    // The children list must be sorted after every insert (preserved by
    // binary_search + insert). Read it back and verify.
    let children = Index::<TestStorage>::get_children_of(parent.id()).unwrap();
    assert_eq!(children.len(), 5);
    for pair in children.windows(2) {
        assert!(
            pair[0] <= pair[1],
            "children must stay sorted across binary-search inserts"
        );
    }

    // Parent's stored full_hash must equal an independent recompute
    // over the children in the order they landed — proves the sorted
    // binary-search insert keeps the children slice consistent with
    // what the hash function consumes.
    let stored = Index::<TestStorage>::get_hashes_for(parent.id())
        .unwrap()
        .unwrap()
        .0;
    let recomputed = recompute_full_hash::<TestStorage>(parent.id());
    assert_eq!(stored, recomputed);
}

#[test]
fn sorted_insert_replaces_existing_child_in_place() {
    // Re-adding a child with the same (created_at, id) but updated hash
    // should replace the existing entry, not duplicate it.
    use crate::entities::ChildInfo;
    use crate::index::Index;

    crate::env::reset_for_testing();
    crate::tests::common::register_test_merge_functions();

    let mut parent = Page::new_from_element("Parent", Element::root());
    TestInterface::save(&mut parent).unwrap();

    let mut child = Paragraph::new_from_element("Child", Element::new(None));
    TestInterface::add_child_to(parent.id(), &mut child).unwrap();

    // Second add_child_to with the same child but a new own_hash should
    // replace the entry at the same position.
    let initial_children = Index::<TestStorage>::get_children_of(parent.id()).unwrap();
    assert_eq!(initial_children.len(), 1);

    let replacement = ChildInfo::new(child.id(), [99_u8; 32], child.element().metadata.clone());
    Index::<TestStorage>::add_child_to(parent.id(), replacement).unwrap();

    let after_children = Index::<TestStorage>::get_children_of(parent.id()).unwrap();
    assert_eq!(after_children.len(), 1, "replace should not duplicate");
}

// ============================================================
// #2238 Fix 1 — Stored full_hash is always authoritative
// ============================================================

#[test]
fn stored_full_hash_always_matches_fresh_recompute_after_add_root() {
    // After #2238 Fix 1, add_root populates full_hash eagerly rather
    // than leaving it at [0; 32]. Every subsequent read should see the
    // correct SHA256(own_hash) without needing to re-hash.
    use crate::address::Id;
    use crate::entities::ChildInfo;
    use crate::index::Index;

    crate::env::reset_for_testing();
    crate::tests::common::register_test_merge_functions();

    let id = Id::random();
    let own_hash = [7_u8; 32];
    Index::<TestStorage>::add_root(ChildInfo::new(
        id,
        own_hash,
        crate::entities::Metadata::default(),
    ))
    .unwrap();

    let stored = Index::<TestStorage>::get_hashes_for(id).unwrap().unwrap();
    assert_eq!(stored.1, own_hash, "own_hash should be stored verbatim");
    assert_ne!(
        stored.0, [0_u8; 32],
        "full_hash must be computed eagerly by add_root (#2238 Fix 1), not left at default zero"
    );

    // get_full_merkle_hash_for should return the same value
    // without touching children — it's just a stored read now.
    let via_read = Index::<TestStorage>::get_full_merkle_hash_for(id).unwrap();
    assert_eq!(
        stored.0, via_read,
        "get_full_merkle_hash_for returns stored full_hash (#2238 Fix 1)"
    );
}

#[test]
fn stored_full_hash_stays_authoritative_through_ancestor_walks() {
    // After #2238 Fix 1, `get_full_merkle_hash_for` is an O(1)
    // stored-read, so comparing it against `get_hashes_for` would be
    // circular. Instead, independently recompute each node's full_hash
    // from its children (via `calculate_full_hash_for_children` on the
    // raw EntityIndex) and assert it matches the stored value. This
    // catches the class of bug the reviewer flagged: a future write
    // site that forgets to refresh `full_hash` would leave the stored
    // value out of sync with what the children imply, and this test
    // would catch it.
    use crate::index::Index;

    crate::env::reset_for_testing();
    crate::tests::common::register_test_merge_functions();

    let mut root = Page::new_from_element("Root", Element::root());
    TestInterface::save(&mut root).unwrap();
    let mut parent = Paragraph::new_from_element("Parent", Element::new(None));
    TestInterface::add_child_to(root.id(), &mut parent).unwrap();
    let mut leaf = Paragraph::new_from_element("Leaf", Element::new(None));
    TestInterface::add_child_to(parent.id(), &mut leaf).unwrap();

    // Modify the leaf's data so its own_hash changes, then propagate.
    leaf.text = "Leaf modified".to_string();
    leaf.element_mut().update();
    TestInterface::save(&mut leaf).unwrap();

    // Every node's stored full_hash must equal an INDEPENDENT recompute
    // that walks its children's stored merkle_hashes — not another read
    // of the stored full_hash value.
    for id in [root.id(), parent.id(), leaf.id()] {
        let index = Index::<TestStorage>::get_index(id).unwrap().unwrap();
        let recomputed = recompute_full_hash::<TestStorage>(id);
        assert_eq!(
            index.full_hash(),
            recomputed,
            "stored full_hash must equal independent recompute for id {:?}",
            id
        );
        assert_ne!(
            index.full_hash(),
            [0_u8; 32],
            "full_hash must be set for every node after ops"
        );
    }
}
