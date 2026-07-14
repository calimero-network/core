//! Shared test helpers for `group_store` unit tests.
//!
//! Extracted from `tests.rs` so the membership-specific test module
//! (`membership/tests.rs`, added in #2306) can share the same setup
//! without duplicating fixtures. Crate-internal: visible to all
//! submodules under `group_store/`, invisible outside.

use super::{MembershipRepository, MetaRepository, NamespaceRepository};
use std::sync::Arc;

use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::db::InMemoryDB;
use calimero_store::key::{GroupMetaValue, GroupParentRef};
use calimero_store::Store;
use rand::rngs::OsRng;
pub(super) fn test_store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

pub(super) fn test_group_id() -> ContextGroupId {
    ContextGroupId::from([0xAA; 32])
}

/// Build a `MemberRemoved` op with placeholder cross-DAG claims for
/// tests that don't exercise the convergence-detection path. The
/// claims here are deliberately zero/empty so a receiver verifying
/// against actual post-apply state will see a mismatch — tests that
/// hit the apply path either ignore the mismatch (it's a warn-log,
/// not a hard reject) or use the real `compute_*` helpers.
pub(super) fn dummy_member_removed_op(member: PublicKey) -> GroupOp {
    GroupOp::MemberRemoved {
        member,
        expected_group_state_hash: [0u8; 32],
        expected_context_state_hashes: Vec::new(),
    }
}

pub(super) fn test_meta() -> GroupMetaValue {
    GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: PublicKey::from([0x01; 32]),
        owner_identity: PublicKey::from([0x01; 32]),
        migration: None,
        auto_join: true,
    }
}

/// Variant of [`test_meta`] that wires both the admin and owner identity to
/// the supplied key. Used by tests that want a specific admin pubkey.
pub(super) fn sample_meta_with_admin(admin: PublicKey) -> GroupMetaValue {
    GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    }
}

/// Bootstrap a namespace root with a freshly-generated admin: writes the
/// root meta (`admin == owner`), an `Admin` member row, and the admin's
/// stored identity. Returns the admin's `(PrivateKey, PublicKey)` so the
/// caller can sign ops and seed subgroup metas. Collapses the
/// meta-save + add_member + store_identity setup duplicated across the
/// namespace apply tests.
pub(super) fn bootstrap_namespace_with_admin(
    store: &Store,
    ns_id: [u8; 32],
) -> (PrivateKey, PublicKey) {
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut OsRng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();
    let ns_gid = ContextGroupId::from(ns_id);
    MetaRepository::new(store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();
    (admin_sk, admin_pk)
}

/// Shortcut for nesting one group under another inside tests, unwrapping
/// the result. Used by membership-path tests across both `tests.rs` and
/// `membership/tests.rs`.
pub(super) fn nest_for_test(store: &Store, parent: &ContextGroupId, child: &ContextGroupId) {
    NamespaceRepository::new(store).nest(parent, child).unwrap();
}

/// Like [`nest_for_test`] but writes the parent edge directly to the
/// store, bypassing `NamespaceRepository::nest`'s `MAX_NAMESPACE_DEPTH`
/// guard. Used by tests that need to construct chains longer than the
/// walkers tolerate (depth-overflow regression tests for
/// `enumerate_inherited`, `is_open_chain_to_namespace`, etc.). The
/// resulting tree is intentionally malformed from the production API's
/// perspective — only the walker bail-out path should ever observe it.
///
/// **Asymmetric edge.** Only writes the child→parent `GroupParentRef`
/// edge. The parent→child `GroupChildIndex` edge that real `nest`
/// writes is *not* set, so `list_children` / `collect_descendants` /
/// any downward walk will not see these synthetic edges. Use this
/// helper only for tests that walk upward (resolve, check_path,
/// is_open_chain_to_namespace, enumerate_inherited).
pub(super) fn nest_for_test_unchecked(
    store: &Store,
    parent: &ContextGroupId,
    child: &ContextGroupId,
) {
    let mut handle = store.handle();
    handle
        .put(&GroupParentRef::new(child.to_bytes()), &parent.to_bytes())
        .unwrap();
}
