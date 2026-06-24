//! Fold-equivalence: the unified-op **projection** must resolve the same
//! membership the **live** governance resolver does, across the open-subgroup
//! INHERITANCE lifecycle (join → inherit → remove-from-root revokes).
//!
//! This is the deterministic harness that drives the grant-direction fidelity
//! work (the e2e `group-remove-from-root-revokes-inherited` over-grant, reduced
//! to a unit test): one store is the live reference (`MembershipRepository` /
//! `check_path`); the same ops are folded into a `ScopeProjections` and read via
//! `member_at_cut`. Any divergence between live and projection fails here —
//! no CI roulette.

use std::sync::Arc;

use calimero_context::group_store::{
    self, CapabilitiesRepository, MembershipRepository, MetaRepository, NamespaceRepository,
};
use calimero_context::scope_projection::{op_from_namespace_op, ScopeProjections};
use calimero_context_client::local_governance::{
    EncryptedGroupOp, GroupOp, NamespaceOp, RootOp, SignedNamespaceOp,
};
use calimero_context_config::types::{
    ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
};
use calimero_context_config::{MemberCapabilities, VisibilityMode};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_store::db::InMemoryDB;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use core::num::NonZeroU128;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

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

fn sign_invitation(
    admin_sk: &PrivateKey,
    group: ContextGroupId,
    role: u8,
) -> SignedGroupOpenInvitation {
    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*admin_sk.public_key().digest()),
        group_id: group,
        // Far-future absolute expiry: a small relative value resolves to ~1971 as
        // an absolute wall-clock and would silently break these tests the day the
        // apply path starts enforcing invitation expiry.
        expiration_timestamp: u64::MAX,
        secret_salt: [0x42; 32],
        invited_role: role,
    };
    let inv_bytes = borsh::to_vec(&invitation).expect("borsh invitation");
    let inv_sig = admin_sk
        .sign(&Sha256::digest(&inv_bytes))
        .expect("sign invitation");
    SignedGroupOpenInvitation {
        invitation,
        inviter_signature: hex::encode(inv_sig.to_bytes()),
        application_id: None,
        app_key: None,
    }
}

/// Fold the structural ops the projection needs for the inheritance walk — the
/// subgroup TREE (`GroupCreated`) and its OPEN visibility (`SubgroupVisibilitySet`)
/// — into `proj`, chained from `prev` (genesis when `None`). In production these
/// are emitted as governance ops; this mirrors that so the projection's
/// `subgroups` map is populated (live gets the same state via repo writes in the
/// test setup). Returns the id of the last folded op (to chain the next from).
fn fold_subgroup_structure(
    proj: &mut ScopeProjections,
    namespace: [u8; 32],
    admin: PublicKey,
    subgroup: ContextGroupId,
    created_id: [u8; 32],
    visibility_id: [u8; 32],
) -> [u8; 32] {
    let created = SignedNamespaceOp {
        version: 1,
        namespace_id: namespace,
        parent_op_hashes: Vec::new(),
        state_hash: [0u8; 32],
        signer: admin,
        nonce: 0,
        op: NamespaceOp::Root(RootOp::GroupCreated {
            group_id: subgroup.to_bytes(),
            parent_id: namespace,
            restricted: true,
        }),
        signature: [0u8; 64],
    };
    proj.ingest_op(&op_from_namespace_op(
        &created,
        None,
        created_id,
        hlc(0),
        &[],
    ));
    let vis = ns_group_envelope(namespace, admin, subgroup);
    proj.ingest_op(&op_from_namespace_op(
        &vis,
        Some(&GroupOp::SubgroupVisibilitySet { mode: 0 }), // 0 = Open
        visibility_id,
        hlc(0),
        &[created_id],
    ));
    visibility_id
}

/// A `NamespaceOp::Group` envelope for folding; the cleartext op is supplied
/// separately to `op_from_namespace_op` (the projection decrypts post-apply).
fn ns_group_envelope(
    namespace: [u8; 32],
    signer: PublicKey,
    group: ContextGroupId,
) -> SignedNamespaceOp {
    SignedNamespaceOp {
        version: 1,
        namespace_id: namespace,
        parent_op_hashes: Vec::new(),
        state_hash: [0u8; 32],
        signer,
        nonce: 0,
        op: NamespaceOp::Group {
            group_id: group.to_bytes(),
            key_id: [0u8; 32],
            encrypted: EncryptedGroupOp {
                nonce: [0u8; 12],
                ciphertext: Vec::new(),
            },
            key_rotation: None,
        },
        signature: [0u8; 64],
    }
}

/// The scenario reduced to a unit test: an OPEN subgroup, a member who inherits
/// access from the namespace root (`CAN_JOIN_OPEN_SUBGROUPS`) via
/// `MemberJoinedOpen` (no persistent direct row), then is removed from the root.
/// Live revokes the subgroup access (no anchor); the projection must too.
#[test]
fn projection_matches_live_across_inherited_join_and_root_removal() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();

    let ns = ContextGroupId::from([0x11; 32]);
    let subgroup = ContextGroupId::from([0x22; 32]);

    // Genesis base state (store seeds, NOT ops — exactly as create_group writes):
    // root + subgroup meta/admin, the subgroup nested + Open, root default cap.
    for g in [&ns, &subgroup] {
        MetaRepository::new(&store).save(g, &meta(admin)).unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();

    let mut proj = ScopeProjections::new();
    // Fold the subgroup tree + Open visibility so the inheritance walk can
    // traverse subgroup → root (the structural ops the projection needs).
    let s2 = fold_subgroup_structure(
        &mut proj,
        ns.to_bytes(),
        admin,
        subgroup,
        [0xA0; 32],
        [0xAF; 32],
    );

    // (1) joiner joins the namespace root via invitation — a DIRECT membership.
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes(),
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: joiner,
            signed_invitation: sign_invitation(&admin_sk, ns, 1),
        }),
    )
    .expect("sign join_ns");
    group_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    let id1 = [0xA1; 32];
    proj.ingest_op(&op_from_namespace_op(&join_ns, None, id1, hlc(1), &[s2]));

    // (2) joiner joins the OPEN subgroup via inheritance (MemberJoinedOpen) —
    // live writes NO direct row; membership is re-derived from the anchor.
    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes(),
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: joiner,
            group_id: subgroup.to_bytes(),
        }),
    )
    .expect("sign join_sub");
    group_store::apply_signed_namespace_op(&store, &join_sub).unwrap();
    let id2 = [0xA2; 32];
    proj.ingest_op(&op_from_namespace_op(&join_sub, None, id2, hlc(2), &[id1]));

    // After the joins: both authorities must see the joiner in the subgroup
    // (live by inheritance walk; projection likewise).
    let live_member_after_join = MembershipRepository::new(&store)
        .is_member(&subgroup, &joiner)
        .unwrap();
    assert!(
        live_member_after_join,
        "live: joiner inherits subgroup access"
    );
    // This `Some(true)` is driven by the at-cut FOLD, not the materialized
    // fallback: `MemberJoinedOpen` writes no direct subgroup row, so
    // `member_at_cut`'s `role_of(subgroup, joiner)` fallback finds nothing and the
    // inheritance walk over the folded ancestry is what must resolve membership.
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &joiner, &[id2]),
        Some(true),
        "projection must agree the joiner is a member after the inherited join"
    );
    // The GRANT resolver (sole-authority path) must also see the member: complete
    // ancestry is folded, so it returns the at-cut verdict.
    assert_eq!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[id2]),
        Some(true),
        "authoritative grant resolver agrees the joiner is a member after the join"
    );

    // (3) admin removes the joiner from the NAMESPACE ROOT only (a GroupOp on the
    // root). Live: removes the root row; the subgroup has no direct row, so the
    // inheritance walk now finds no anchor → revoked.
    //
    // The store side uses the repo write directly rather than
    // `apply_signed_namespace_op`: a `MemberRemoved` is an ENCRYPTED `GroupOp`, and
    // round-tripping it through the signed-apply path needs a real per-group key +
    // ciphertext this harness doesn't model (the joins above are cleartext
    // `RootOp`s, hence applied through the signed path). The equivalence under test
    // is fold-vs-materialized-membership; `remove_member` yields the same
    // `is_member` result a real node's apply would, which is all `is_member` reads.
    let removal = GroupOp::MemberRemoved {
        member: joiner,
        expected_group_state_hash: [0u8; 32],
        expected_context_state_hashes: Vec::new(),
    };
    MembershipRepository::new(&store)
        .remove_member(&ns, &joiner)
        .unwrap();
    let id3 = [0xA3; 32];
    let removal_env = ns_group_envelope(ns.to_bytes(), admin, ns);
    proj.ingest_op(&op_from_namespace_op(
        &removal_env,
        Some(&removal),
        id3,
        hlc(3),
        &[id2],
    ));

    // THE equivalence: after root removal, live revokes the inherited subgroup
    // access; the projection must NOT keep granting it (the over-grant).
    let live_member_after_removal = MembershipRepository::new(&store)
        .is_member(&subgroup, &joiner)
        .unwrap();
    assert!(
        !live_member_after_removal,
        "live: removing from root revokes inherited subgroup access"
    );
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &joiner, &[id3]),
        Some(false),
        "projection must revoke the inherited subgroup access after root removal \
         (matching live) — granting here is the over-grant"
    );
    // The GRANT resolver MUST NOT grant where live rejects — this is the
    // sole-authority safety property (it can never over-authorize).
    assert_ne!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[id3]),
        Some(true),
        "authoritative grant resolver must NOT grant a write live rejected (over-grant)"
    );
}

/// Symmetric guard for the UNDER-grant: a member who leaves the namespace and
/// then REJOINS must regain inherited subgroup access — both in live and in the
/// projection. This isolates walk-logic correctness (all ops folded here) from
/// the e2e feed-completeness gap that previously broke this case.
#[test]
fn projection_matches_live_across_leave_and_rejoin_inheritance() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();

    let ns = ContextGroupId::from([0x31; 32]);
    let subgroup = ContextGroupId::from([0x32; 32]);

    for g in [&ns, &subgroup] {
        MetaRepository::new(&store).save(g, &meta(admin)).unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();

    let mut proj = ScopeProjections::new();
    let s2 = fold_subgroup_structure(
        &mut proj,
        ns.to_bytes(),
        admin,
        subgroup,
        [0xB0; 32],
        [0xBF; 32],
    );

    // join ns (nonce 1) + inherit subgroup (nonce 2).
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes(),
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: joiner,
            signed_invitation: sign_invitation(&admin_sk, ns, 1),
        }),
    )
    .unwrap();
    group_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &join_ns,
        None,
        [0xB1; 32],
        hlc(1),
        &[s2],
    ));

    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes(),
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: joiner,
            group_id: subgroup.to_bytes(),
        }),
    )
    .unwrap();
    group_store::apply_signed_namespace_op(&store, &join_sub).unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &join_sub,
        None,
        [0xB2; 32],
        hlc(2),
        &[[0xB1; 32]],
    ));

    // leave ns: remove the root row (GroupOp on root, folded).
    let leave = GroupOp::MemberRemoved {
        member: joiner,
        expected_group_state_hash: [0u8; 32],
        expected_context_state_hashes: Vec::new(),
    };
    MembershipRepository::new(&store)
        .remove_member(&ns, &joiner)
        .unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &ns_group_envelope(ns.to_bytes(), admin, ns),
        Some(&leave),
        [0xB3; 32],
        hlc(3),
        &[[0xB2; 32]],
    ));
    // After leaving: not a member (both).
    assert!(!MembershipRepository::new(&store)
        .is_member(&subgroup, &joiner)
        .unwrap());
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &joiner, &[[0xB3; 32]]),
        Some(false)
    );

    // REJOIN ns via invitation (direct root membership again).
    let rejoin_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes(),
        vec![],
        [0u8; 32],
        3,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: joiner,
            signed_invitation: sign_invitation(&admin_sk, ns, 1),
        }),
    )
    .unwrap();
    group_store::apply_signed_namespace_op(&store, &rejoin_ns).unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &rejoin_ns,
        None,
        [0xB4; 32],
        hlc(4),
        &[[0xB3; 32]],
    ));

    // After rejoin: inherited subgroup access is restored — both authorities.
    let live = MembershipRepository::new(&store)
        .is_member(&subgroup, &joiner)
        .unwrap();
    assert!(
        live,
        "live: rejoining the root restores inherited subgroup access"
    );
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &joiner, &[[0xB4; 32]]),
        Some(true),
        "projection must restore inherited access on rejoin (the under-grant guard)"
    );
    // The GRANT resolver also restores access on rejoin (complete ancestry folded).
    assert_eq!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[[0xB4; 32]]),
        Some(true),
        "authoritative grant resolver restores inherited access on rejoin"
    );
}

/// Backfill-lag deferral: when the cut's ancestry is only PARTIALLY folded — the
/// write arrived before a proactive governance backfill folded the author's
/// membership chain — the deny co-authorizer must DEFER to live (`None`), not
/// reject (`Some(false)`). An inherited open-subgroup membership is the exposed
/// case: deriving it needs the whole chain folded (anchor membership + subgroup
/// edge + visibility + cap), so a truncated fold reads not-a-member. Live (with
/// its materialized rows) still authorizes; the projection must not contradict it
/// on a partial view. This reproduces the single transient divergence the e2e
/// cutover gate caught in `group-leave-then-rejoin-via-inheritance` (one marker,
/// emitted mid-backfill, gone once the ancestry completed).
#[test]
fn projection_defers_when_cut_ancestry_incomplete() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();

    let ns = ContextGroupId::from([0x41; 32]);
    let subgroup = ContextGroupId::from([0x42; 32]);

    for g in [&ns, &subgroup] {
        MetaRepository::new(&store).save(g, &meta(admin)).unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();

    // LIVE applies the full chain — root join + inherited subgroup join — so the
    // live resolver authoritatively sees the joiner as an inherited member.
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes(),
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: joiner,
            signed_invitation: sign_invitation(&admin_sk, ns, 1),
        }),
    )
    .unwrap();
    group_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes(),
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: joiner,
            group_id: subgroup.to_bytes(),
        }),
    )
    .unwrap();
    group_store::apply_signed_namespace_op(&store, &join_sub).unwrap();
    assert!(
        MembershipRepository::new(&store)
            .is_member(&subgroup, &joiner)
            .unwrap(),
        "live: joiner inherits subgroup access"
    );

    // The PROJECTION has only folded the subgroup structure + the rejoin op
    // itself, NOT the joiner's root-membership ancestor (`[0xC1; 32]` is never
    // ingested) — exactly the mid-backfill state on the node that caught the
    // divergence. The cited head `[0xC2; 32]` is present, but its ancestry is
    // truncated, so the inheritance walk can find no anchor membership.
    let mut proj = ScopeProjections::new();
    fold_subgroup_structure(
        &mut proj,
        ns.to_bytes(),
        admin,
        subgroup,
        [0xC0; 32],
        [0xCF; 32],
    );
    proj.ingest_op(&op_from_namespace_op(
        &join_sub,
        None,
        [0xC2; 32],
        hlc(2),
        &[[0xC1; 32]], // parent (the root join) deliberately NOT ingested
    ));

    // Pre-fix this returned `Some(false)` (the walk fails + no direct row for the
    // materialized fallback) → a false deny that tripped the cutover gate. The
    // completeness guard makes it abstain instead.
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &joiner, &[[0xC2; 32]]),
        None,
        "projection must DEFER to live (None) on a partially-folded cut, not deny"
    );
}
