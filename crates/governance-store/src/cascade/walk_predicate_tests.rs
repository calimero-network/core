//! Unit tests for [`super::walk_for_predicate`] predicate equality
//! + signed-group inclusion.
//!
//! These fixtures stand up a minimal in-memory namespace by writing
//! `GroupMetaValue` rows directly and stitching parent edges via the
//! public [`nest_group`] helper, so the tests exercise the walk against
//! the same store-shape contract production hits at apply time without
//! depending on any test-only helpers internal to `group_store`.

use std::sync::Arc;

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_store::db::InMemoryDB;
use calimero_store::key::{GroupMetaValue, GroupTarget};
use calimero_store::Store;

use super::walk_for_predicate;
use crate::{MetaRepository, NamespaceRepository};

const BYTECODE_ID_A: [u8; 32] = [0xA1; 32];
const BYTECODE_ID_B: [u8; 32] = [0xB2; 32];

fn test_store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn group_id(byte: u8) -> ContextGroupId {
    ContextGroupId::from([byte; 32])
}

fn meta_with_bytecode_id(bytecode_id: [u8; 32]) -> GroupMetaValue {
    GroupMetaValue {
        target: GroupTarget {
            application_id: ApplicationId::from([0xCC; 32]),
            bytecode_id,
            package: Box::default(),
            version: Box::default(),
        },
        created_at: 1_700_000_000,
        admin_identity: AccountId::from([0x01; 32]),
        owner_identity: AccountId::from([0x01; 32]),
        migration: None,
        auto_join: true,
    }
}

/// Build `root` with two direct children, every group on `bytecode_id`.
fn fixture_homogeneous_tree(
    store: &Store,
    root: ContextGroupId,
    child_a: ContextGroupId,
    child_b: ContextGroupId,
    bytecode_id: [u8; 32],
) {
    MetaRepository::new(store)
        .save(&root, &meta_with_bytecode_id(bytecode_id))
        .unwrap();
    MetaRepository::new(store)
        .save(&child_a, &meta_with_bytecode_id(bytecode_id))
        .unwrap();
    MetaRepository::new(store)
        .save(&child_b, &meta_with_bytecode_id(bytecode_id))
        .unwrap();
    NamespaceRepository::new(store)
        .nest(&root, &child_a)
        .unwrap();
    NamespaceRepository::new(store)
        .nest(&root, &child_b)
        .unwrap();
}

#[test]
fn predicate_match_includes_descendant() {
    let store = test_store();
    let root = group_id(0xA0);
    let child_a = group_id(0xA1);
    let child_b = group_id(0xA2);
    fixture_homogeneous_tree(&store, root, child_a, child_b, BYTECODE_ID_A);

    let entries = walk_for_predicate(&store, root, BYTECODE_ID_A).unwrap();

    assert_eq!(
        entries.len(),
        3,
        "walk must emit root + 2 children, got {entries:?}"
    );
    assert!(
        entries.iter().all(|e| e.matched),
        "every entry must match when bytecode_id is uniform across the tree: {entries:?}"
    );

    // Membership check — every fixture group must appear, order-agnostic.
    let emitted: std::collections::HashSet<_> = entries.iter().map(|e| e.group_id).collect();
    assert!(emitted.contains(&root));
    assert!(emitted.contains(&child_a));
    assert!(emitted.contains(&child_b));
}

#[test]
fn predicate_mismatch_skips_descendant() {
    let store = test_store();
    let root = group_id(0xC0);
    let child_a = group_id(0xC1); // bytecode_id A — should match
    let child_b = group_id(0xC2); // bytecode_id B — should NOT match

    MetaRepository::new(&store)
        .save(&root, &meta_with_bytecode_id(BYTECODE_ID_A))
        .unwrap();
    MetaRepository::new(&store)
        .save(&child_a, &meta_with_bytecode_id(BYTECODE_ID_A))
        .unwrap();
    MetaRepository::new(&store)
        .save(&child_b, &meta_with_bytecode_id(BYTECODE_ID_B))
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&root, &child_a)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&root, &child_b)
        .unwrap();

    let entries = walk_for_predicate(&store, root, BYTECODE_ID_A).unwrap();
    assert_eq!(entries.len(), 3, "walk must visit every group: {entries:?}");

    // The B-child must be present but marked `matched = false`.
    let b_entry = entries
        .iter()
        .find(|e| e.group_id == child_b)
        .expect("B-child must appear in walk output even though it skips");
    assert!(
        !b_entry.matched,
        "B-child has bytecode_id B but predicate is from_bytecode_id=A — must not match"
    );

    // The A-child + root must match.
    let a_entry = entries
        .iter()
        .find(|e| e.group_id == child_a)
        .expect("A-child must appear");
    assert!(a_entry.matched, "A-child has bytecode_id A — must match");
    let root_entry = entries
        .iter()
        .find(|e| e.group_id == root)
        .expect("root must appear");
    assert!(root_entry.matched, "root has bytecode_id A — must match");
}

#[test]
fn walk_includes_signed_group() {
    // Even with no descendants, the signed group itself must always be
    // emitted — it's the root of the cascade and the apply handler
    // depends on it appearing in the walk to mutate the signed group's
    // own settings.
    let store = test_store();
    let root = group_id(0xE0);
    MetaRepository::new(&store)
        .save(&root, &meta_with_bytecode_id(BYTECODE_ID_A))
        .unwrap();

    let entries = walk_for_predicate(&store, root, BYTECODE_ID_A).unwrap();

    assert_eq!(
        entries.len(),
        1,
        "signed group alone yields exactly 1 entry"
    );
    assert_eq!(entries[0].group_id, root);
    assert!(
        entries[0].matched,
        "root with matching bytecode_id must match"
    );
}

#[test]
fn walk_emits_signed_group_when_meta_missing() {
    // A signed group whose `GroupMeta` row hasn't been materialized
    // yet (e.g. a fresh peer that hasn't caught up on the namespace
    // governance DAG) is still emitted — but with `matched = false`,
    // so the cascade apply arm correctly skips writing against a row
    // that isn't there.
    let store = test_store();
    let root = group_id(0xF0);

    let entries = walk_for_predicate(&store, root, BYTECODE_ID_A).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].group_id, root);
    assert!(
        !entries[0].matched,
        "missing GroupMeta must be treated as predicate-miss, not as a hard error"
    );
}
