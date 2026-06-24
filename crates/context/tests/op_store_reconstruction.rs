//! Deterministic before/after for the C2.2b read-flip blocker + its C2.2c fix.
//!
//! The unified op-store must reconstruct the SAME namespace-governance membership
//! that `collect_namespace_ops` (the governance-DAG walk) does — otherwise making
//! the op-store the projection's authoritative backing silently drops members.
//!
//! The bug: an encrypted group `MemberAdded` can be applied to the governance DAG
//! BEFORE its group key arrives (keys travel on a separate delivery/pull path and
//! lag — see the buffer-awaiting-key replay in `apply_group_op_inner`). At that
//! moment the apply-time dual-write decrypts with no key, so `op_from_namespace_op`
//! folds the op as `Noop` and that `Noop` is frozen into the op-store. Once the key
//! lands, `collect_namespace_ops` re-decrypts the SAME op at read time and recovers
//! the real `MemberAdded` — but the op-store still held the frozen `Noop`, so a
//! projection read off the op-store loses the membership, the new member's writes
//! are rejected, and sync never converges (the joiner-write timeout seen across
//! group-3node / scaffolding-e2e / shared-storage-rotation when the flip was live).
//! `scope_root` doesn't catch it: at shadow time neither side has decrypted yet
//! (both `Noop` → roots match); the divergence only appears post-key.
//!
//! The fix (`ScopeProjections::repersist_namespace_ops`, called on the
//! key-delivery path): once the key lands, re-walk the namespace with current
//! decryption and re-persist every op, overwriting the frozen `Noop` with the real
//! `MemberAdded`. This test asserts the op-store diverges BEFORE the re-persist and
//! converges AFTER.

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
fn op_store_reconstruction_recovers_late_decrypted_membership_after_key_delivery() {
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

    let heads = [delta_id];

    // Read membership from a fresh fold of whatever the op-store currently holds.
    let store_membership = |store: &Store| {
        let store_ops = load_scope_ops(store, &ScopeId::from(ns_bytes)).unwrap();
        let mut proj = ScopeProjections::new();
        proj.apply_backfill(ns_bytes, store_ops);
        proj.member_at_cut(store, ns, &member, &heads)
    };

    // The governance-DAG walk re-decrypts (key present) and sees the member — this
    // is the truth the op-store must match.
    let dag_ops = ScopeProjections::collect_namespace_ops(&store, ns_bytes).unwrap();
    let mut p_dag = ScopeProjections::new();
    p_dag.apply_backfill(ns_bytes, dag_ops);
    let dag_member = p_dag.member_at_cut(&store, ns, &member, &heads);
    assert_eq!(
        dag_member,
        Some(true),
        "governance-DAG reconstruction should re-decrypt the MemberAdded and see the member"
    );

    // BEFORE the fix: the op-store froze a Noop, so its reconstruction drops the
    // member — the joiner-write divergence that broke sync when the flip was live.
    assert_eq!(
        store_membership(&store),
        Some(false),
        "precondition: the op-store froze the encrypted MemberAdded as a Noop"
    );

    // THE FIX: a key delivery re-persists the namespace's ops with current
    // decryption, overwriting the frozen Noop with the real MemberAdded.
    ScopeProjections::repersist_namespace_ops(&store, ns_bytes);

    // AFTER: the op-store reconstruction now matches the governance-DAG fold.
    assert_eq!(
        store_membership(&store),
        dag_member,
        "after key delivery the op-store must reconstruct the same membership as the governance \
         DAG; repersist_namespace_ops should have overwritten the frozen Noop with the MemberAdded"
    );
}

/// The case the fresh-projection test above MISSED, which broke the e2e: a
/// **maintained** projection that already folded the op as `Noop` (live apply
/// before the key arrived). `op.id` is the signed-op hash, so the `Noop` and the
/// decrypted form share an id; the op-log dedups by id, so re-ingesting the
/// corrected op via a later backfill must UPGRADE the stale `Noop` entry — not be
/// dropped as a duplicate — or `member_at_cut` (which folds the op-log) stays
/// stuck on the `Noop` and the joiner's writes are rejected forever.
#[test]
fn maintained_projection_recovers_late_decrypted_membership_on_backfill() {
    let store = store();
    let admin = PrivateKey::random(&mut OsRng).public_key();
    let member = PrivateKey::random(&mut OsRng).public_key();

    let ns = ContextGroupId::from([0x11; 32]);
    let ns_bytes = ns.to_bytes();

    MetaRepository::new(&store).save(&ns, &meta(admin)).unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns, &admin, GroupMemberRole::Admin)
        .unwrap();
    let group_key = [0x5A; 32];
    let key_id = GroupKeyring::new(&store, ns).store_key(&group_key).unwrap();

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
    NamespaceOpLogService::new(&store, ns_bytes)
        .store_signed_operation(&signed)
        .unwrap();
    NamespaceDagService::new(&store, ns_bytes)
        .advance_dag_head(delta_id, &[], 0)
        .unwrap();

    let heads = [delta_id];

    // A MAINTAINED projection that first folded the op WITHOUT the key (the live
    // apply path before key delivery): the op-log records a `Noop`. The op-store
    // froze the same `Noop` via the dual-write.
    let mut proj = ScopeProjections::new();
    let frozen = op_from_namespace_op(&signed, None, delta_id, hlc(1), &[]);
    proj.ingest_op(&frozen);
    persist_op(&store, &frozen).unwrap();
    assert_eq!(
        proj.member_at_cut(&store, ns, &member, &heads),
        Some(false),
        "precondition: the maintained projection folded the op as a Noop"
    );

    // Key delivery: the op-store is re-persisted with current decryption, then the
    // projection backfills from it — exactly the production sequence.
    ScopeProjections::repersist_namespace_ops(&store, ns_bytes);
    let ops = ScopeProjections::ops_for_namespace(&store, ns_bytes).unwrap();
    proj.apply_backfill(ns_bytes, ops);

    // The re-ingested op must UPGRADE the stale Noop in the op-log, not dedup away.
    assert_eq!(
        proj.member_at_cut(&store, ns, &member, &heads),
        Some(true),
        "the maintained projection must recover the member after the corrected op is re-ingested"
    );
}
