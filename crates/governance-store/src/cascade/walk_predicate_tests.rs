//! Unit tests for [`super::walk_for_predicate`] predicate equality
//! + signed-group inclusion.
//!
//! These fixtures stand up a minimal in-memory namespace by writing
//! `GroupMetaValue` rows directly and stitching parent edges via the
//! public [`nest_group`] helper, so the tests exercise the walk against
//! the same store-shape contract production hits at apply time without
//! depending on any test-only helpers internal to `group_store`.

use std::sync::Arc;

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::UpgradePolicy;
use calimero_primitives::identity::PublicKey;
use calimero_store::db::InMemoryDB;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;

use super::walk_for_predicate;
use crate::{MetaRepository, NamespaceRepository};

const APP_KEY_A: [u8; 32] = [0xA1; 32];
const APP_KEY_B: [u8; 32] = [0xB2; 32];

fn test_store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn group_id(byte: u8) -> ContextGroupId {
    ContextGroupId::from([byte; 32])
}

fn meta_with_app_key(app_key: [u8; 32]) -> GroupMetaValue {
    GroupMetaValue {
        app_key,
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: PublicKey::from([0x01; 32]),
        owner_identity: PublicKey::from([0x01; 32]),
        migration: None,
        auto_join: true,
    }
}

/// Build `root` with two direct children, every group on `app_key`.
fn fixture_homogeneous_tree(
    store: &Store,
    root: ContextGroupId,
    child_a: ContextGroupId,
    child_b: ContextGroupId,
    app_key: [u8; 32],
) {
    MetaRepository::new(store)
        .save(&root, &meta_with_app_key(app_key))
        .unwrap();
    MetaRepository::new(store)
        .save(&child_a, &meta_with_app_key(app_key))
        .unwrap();
    MetaRepository::new(store)
        .save(&child_b, &meta_with_app_key(app_key))
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
    fixture_homogeneous_tree(&store, root, child_a, child_b, APP_KEY_A);

    let entries = walk_for_predicate(&store, root, APP_KEY_A).unwrap();

    assert_eq!(
        entries.len(),
        3,
        "walk must emit root + 2 children, got {entries:?}"
    );
    assert!(
        entries.iter().all(|e| e.matched),
        "every entry must match when app_key is uniform across the tree: {entries:?}"
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
    let child_a = group_id(0xC1); // app_key A — should match
    let child_b = group_id(0xC2); // app_key B — should NOT match

    MetaRepository::new(&store)
        .save(&root, &meta_with_app_key(APP_KEY_A))
        .unwrap();
    MetaRepository::new(&store)
        .save(&child_a, &meta_with_app_key(APP_KEY_A))
        .unwrap();
    MetaRepository::new(&store)
        .save(&child_b, &meta_with_app_key(APP_KEY_B))
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&root, &child_a)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&root, &child_b)
        .unwrap();

    let entries = walk_for_predicate(&store, root, APP_KEY_A).unwrap();
    assert_eq!(entries.len(), 3, "walk must visit every group: {entries:?}");

    // The B-child must be present but marked `matched = false`.
    let b_entry = entries
        .iter()
        .find(|e| e.group_id == child_b)
        .expect("B-child must appear in walk output even though it skips");
    assert!(
        !b_entry.matched,
        "B-child has app_key B but predicate is from_app_key=A — must not match"
    );

    // The A-child + root must match.
    let a_entry = entries
        .iter()
        .find(|e| e.group_id == child_a)
        .expect("A-child must appear");
    assert!(a_entry.matched, "A-child has app_key A — must match");
    let root_entry = entries
        .iter()
        .find(|e| e.group_id == root)
        .expect("root must appear");
    assert!(root_entry.matched, "root has app_key A — must match");
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
        .save(&root, &meta_with_app_key(APP_KEY_A))
        .unwrap();

    let entries = walk_for_predicate(&store, root, APP_KEY_A).unwrap();

    assert_eq!(
        entries.len(),
        1,
        "signed group alone yields exactly 1 entry"
    );
    assert_eq!(entries[0].group_id, root);
    assert!(entries[0].matched, "root with matching app_key must match");
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

    let entries = walk_for_predicate(&store, root, APP_KEY_A).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].group_id, root);
    assert!(
        !entries[0].matched,
        "missing GroupMeta must be treated as predicate-miss, not as a hard error"
    );
}
