//! Deterministic repro for the C2.2b read-flip blocker (deferred in PR #2916).
//!
//! The unified op-store must reconstruct the SAME namespace-governance membership
//! that `collect_namespace_ops` (the governance-DAG walk) does — otherwise making
//! the op-store the projection's authoritative backing silently drops members.
//!
//! The bug this pins: an encrypted group `MemberAdded` can be applied to the
//! governance DAG BEFORE its group key arrives (keys travel on a separate
//! delivery/pull path and lag — see the buffer-awaiting-key replay in
//! `apply_group_op_inner`). At that moment the apply-time dual-write decrypts with
//! no key, so `op_from_namespace_op` folds the op as `Noop` and that `Noop` is
//! frozen into the op-store. Once the key lands, `collect_namespace_ops`
//! re-decrypts the SAME op at read time and recovers the real `MemberAdded` — but
//! the op-store still holds the frozen `Noop`. Flipping the projection onto the
//! op-store therefore loses the membership, the new member's writes are rejected,
//! and sync never converges (the joiner-write timeout seen across group-3node /
//! scaffolding-e2e / shared-storage-rotation when the flip was live).
//!
//! `scope_root` alone does NOT catch this: at the moment the C2.2 shadow runs,
//! neither side has decrypted yet (both `Noop`), so the roots match. The
//! divergence only surfaces after the key arrives — which is why only e2e caught
//! it. This test makes it deterministic.
//!
//! Run it with `cargo test -p calimero-context --test op_store_reconstruction --
//! --ignored`. It is `#[ignore]`d (it FAILS today, on purpose) so it doesn't break
//! CI; un-ignore it as the gate for the fix that makes `load_scope_ops` faithfully
//! reconstruct late-decrypted membership.

use std::sync::Arc;

use calimero_context::group_store::{
    GroupKeyring, MembershipRepository, MetaRepository, NamespaceDagService, NamespaceOpLogService,
};
use calimero_context::scope_projection::{op_from_namespace_op, ScopeProjections};
use calimero_context::unified_op_store::{load_scope_ops, persist_op};
use calimero_context_client::local_governance::{GroupOp, NamespaceOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_op::ScopeId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_store::db::InMemoryDB;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use core::num::NonZeroU128;
use rand::rngs::OsRng;

fn store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn hlc(ns: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(ns),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

fn meta(admin: PublicKey) -> GroupMetaValue {
    GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: calimero_primitives::application::ApplicationId::from([0xCC; 32]),
        upgrade_policy: calimero_primitives::context::UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    }
}

#[test]
#[ignore = "reproduces the C2.2b read-flip blocker (PR #2916): the unified op-store freezes a \
            Noop for an encrypted MemberAdded applied before its group key arrives, while \
            collect_namespace_ops re-decrypts it once the key is present. Un-ignore as the gate \
            for the fix that makes load_scope_ops faithfully reconstruct late-decrypted membership."]
fn op_store_reconstruction_matches_governance_dag_for_late_decrypted_membership() {
    let store = store();
    let admin = PrivateKey::random(&mut OsRng).public_key();
    let member = PrivateKey::random(&mut OsRng).public_key();

    let ns = ContextGroupId::from([0x11; 32]);
    let ns_bytes = ns.to_bytes();

    // Genesis base state (store seeds, exactly as create_group writes): the
    // namespace meta + admin membership + the group key so the encrypted op
    // decrypts at READ time.
    MetaRepository::new(&store).save(&ns, &meta(admin)).unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns, &admin, GroupMemberRole::Admin)
        .unwrap();
    let group_key = [0x5A; 32];
    let key_id = GroupKeyring::new(&store, ns).store_key(&group_key).unwrap();

    // The admin adds `member` via an ENCRYPTED group op.
    let inner = GroupOp::MemberAdded {
        member,
        role: GroupMemberRole::Member,
    };
    let encrypted = GroupKeyring::encrypt_op(&group_key, &inner).unwrap();
    let signed = SignedNamespaceOp {
        version: 1,
        namespace_id: ns_bytes,
        parent_op_hashes: Vec::new(),
        state_hash: [0u8; 32],
        signer: admin,
        nonce: 1,
        op: NamespaceOp::Group {
            group_id: ns_bytes,
            key_id,
            encrypted,
            key_rotation: None,
        },
        signature: [0u8; 64],
    };
    let delta_id = signed.content_hash().unwrap();

    // (1) The op lands in the governance DAG (op-log + frontier), exactly as a
    // received op does — this is what `collect_namespace_ops` walks. The group key
    // is present, so the read-time re-decrypt recovers the MemberAdded.
    NamespaceOpLogService::new(&store, ns_bytes)
        .store_signed_operation(&signed)
        .unwrap();
    NamespaceDagService::new(&store, ns_bytes)
        .advance_dag_head(delta_id, &[], 0)
        .unwrap();

    // (2) The op-store holds the FROZEN-Noop the apply-time dual-write persisted
    // when the key had NOT yet arrived (decrypted = None → Noop). Same op id, so the
    // cut lines up; only the decoded payload differs.
    let frozen = op_from_namespace_op(&signed, None, delta_id, hlc(1), &[]);
    persist_op(&store, &frozen).unwrap();

    // Reconstruct the projection BOTH ways the flip could, then read membership at
    // the op's cut.
    let heads = [delta_id];

    let dag_ops = ScopeProjections::collect_namespace_ops(&store, ns_bytes).unwrap();
    let mut p_dag = ScopeProjections::new();
    p_dag.apply_backfill(ns_bytes, dag_ops);
    let dag_member = p_dag.member_at_cut(&store, ns, &member, &heads);

    let store_ops = load_scope_ops(&store, &ScopeId::from(ns_bytes)).unwrap();
    let mut p_store = ScopeProjections::new();
    p_store.apply_backfill(ns_bytes, store_ops);
    let store_member = p_store.member_at_cut(&store, ns, &member, &heads);

    // Sanity: the governance-DAG walk re-decrypts and DOES see the member.
    assert_eq!(
        dag_member,
        Some(true),
        "governance-DAG reconstruction should re-decrypt the MemberAdded and see the member"
    );

    // The invariant the flip needs: the op-store reconstruction must agree. It does
    // NOT today — it froze a Noop, so the member is missing. This is the joiner-write
    // divergence that broke sync when the read-flip was live.
    assert_eq!(
        store_member, dag_member,
        "op-store reconstruction must match the governance-DAG reconstruction; the op-store froze \
         a Noop for the encrypted MemberAdded (key absent at apply) while collect re-decrypted it \
         (key present at read) — flipping onto the op-store drops the member"
    );
}
