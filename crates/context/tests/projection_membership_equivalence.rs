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

use calimero_context::scope_projection::{op_from_namespace_op, ScopeProjections};
use calimero_context_client::local_governance::{
    EncryptedGroupOp, GroupOp, NamespaceOp, RootOp, SignedNamespaceOp,
};
use calimero_context_config::types::{
    ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
};
use calimero_context_config::{MemberCapabilities, VisibilityMode};
use calimero_governance_store::{
    self, CapabilitiesRepository, MembershipRepository, MetaRepository, NamespaceRepository,
};
use calimero_op::ScopeId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_store::db::InMemoryDB;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use core::num::NonZeroU128;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

/// A joiner credential. Note this test asserts the projection's fold matches the
/// LIVE membership resolver — and the projection now keys `MemberAdded` by
/// `cert.account` rather than a key-derived stand-in, so the account here is
/// load-bearing, not filler.
/// The joiner's credential, derived DETERMINISTICALLY from its signing key.
///
/// Deterministic because the op names the account this certifies and later
/// assertions have to name the same one; a fresh random root per call would make
/// every mention a different principal.
fn test_join_account_for(
    sign_pk: &calimero_primitives::identity::PublicKey,
) -> Box<calimero_context_client::local_governance::JoinAccountCredential> {
    calimero_context::test_support::credential(sign_pk)
}

fn store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn hlc(ns: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(ns),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

fn meta(admin: calimero_account::AccountId) -> GroupMetaValue {
    GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: calimero_primitives::application::ApplicationId::from([0xCC; 32]),
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    }
}

/// `nonce` identifies the invitation. It is a parameter rather than a constant
/// because an invitation is spent once an identity joins with it: presenting the
/// same one again after they exit cannot readmit them, so a re-invite has to
/// carry a fresh nonce. Real invitations get a random one per issue.
fn sign_invitation(
    admin_sk: &PrivateKey,
    group: ContextGroupId,
    role: u8,
    nonce: [u8; 32],
) -> SignedGroupOpenInvitation {
    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*admin_sk.public_key().digest()),
        group_id: group,
        // 0 is the canonical "no expiry" sentinel — MemberJoined carries no
        // joined_at, so any non-zero expiration causes the apply gate to reject it.
        expiration_timestamp: 0,
        invitation_nonce: nonce,
        invited_role: role,
    };
    let inv_bytes = borsh::to_vec(&invitation).expect("borsh invitation");
    let inv_sig = admin_sk
        .sign(&Sha256::digest(&inv_bytes))
        .expect("sign invitation");
    SignedGroupOpenInvitation {
        inviter_account: None,
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
        namespace_id: namespace.into(),
        parent_op_hashes: Vec::new(),
        signer: admin,
        nonce: 0,
        op: NamespaceOp::Root(RootOp::GroupCreated {
            admin: calimero_account::AccountId::from([0x5C; 32]),
            group_id: subgroup.to_bytes().into(),
            parent_id: namespace.into(),
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
        Some(&GroupOp::SubgroupVisibilitySet {
            mode: calimero_context_config::VisibilityMode::Open,
        }),
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
        namespace_id: namespace.into(),
        parent_op_hashes: Vec::new(),
        signer,
        nonce: 0,
        op: NamespaceOp::Group {
            group_id: group.to_bytes().into(),
            key_id: [0u8; 32].into(),
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
    // Enrolled at the ROOT: bindings live at the anchor and every reader
    // resolves up to it, so a row written against the subgroup would be
    // invisible the moment it is nested.
    let admin_account = calimero_context::test_support::enrol(&store, &ns, &admin);
    for g in [&ns, &subgroup] {
        MetaRepository::new(&store)
            .save(g, &meta(admin_account))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin_account, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
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
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x42; 32]),
            account: test_join_account_for(&joiner),
        }),
    )
    .expect("sign join_ns");
    calimero_governance_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    let id1 = [0xA1; 32];
    proj.ingest_op(&op_from_namespace_op(&join_ns, None, id1, hlc(1), &[s2]));

    // (2) joiner joins the OPEN subgroup via inheritance (MemberJoinedOpen) —
    // live writes NO direct row; membership is re-derived from the anchor.
    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: calimero_context::test_support::account_for(&joiner),
            group_id: subgroup.to_bytes().into(),
            account: test_join_account_for(&joiner),
        }),
    )
    .expect("sign join_sub");
    calimero_governance_store::apply_signed_namespace_op(&store, &join_sub).unwrap();
    let id2 = [0xA2; 32];
    proj.ingest_op(&op_from_namespace_op(&join_sub, None, id2, hlc(2), &[id1]));

    // After the joins: both authorities must see the joiner in the subgroup
    // (live by inheritance walk; projection likewise).
    let live_member_after_join = MembershipRepository::new(&store)
        .is_member(
            &subgroup,
            &calimero_context::test_support::account_for(&joiner),
        )
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
        member: calimero_context::test_support::account_for(&joiner),
        expected_group_state_hash: [0u8; 32],
        expected_context_state_hashes: Vec::new(),
    };
    MembershipRepository::new(&store)
        .remove_member(&ns, &calimero_context::test_support::account_for(&joiner))
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
        .is_member(
            &subgroup,
            &calimero_context::test_support::account_for(&joiner),
        )
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

    // Enrolled at the ROOT: bindings live at the anchor and every reader
    // resolves up to it, so a row written against the subgroup would be
    // invisible the moment it is nested.
    let admin_account = calimero_context::test_support::enrol(&store, &ns, &admin);
    for g in [&ns, &subgroup] {
        MetaRepository::new(&store)
            .save(g, &meta(admin_account))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin_account, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
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
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x42; 32]),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &join_ns,
        None,
        [0xB1; 32],
        hlc(1),
        &[s2],
    ));

    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: calimero_context::test_support::account_for(&joiner),
            group_id: subgroup.to_bytes().into(),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_sub).unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &join_sub,
        None,
        [0xB2; 32],
        hlc(2),
        &[[0xB1; 32]],
    ));

    // leave ns: remove the root row (GroupOp on root, folded).
    //
    // The live side drops the row with a direct `remove_member` write rather than
    // by applying the op, so no re-entry block is recorded — which is why the
    // rejoin below is admitted at all. That is deliberate: this test is about
    // fold equivalence between the projection and the live resolver, not about
    // re-entry policy. Run the removal through `MemberRemoved` apply and the
    // rejoin would be rejected outright, no invitation able to readmit them;
    // that path is covered in `governance-store`.
    let leave = GroupOp::MemberRemoved {
        member: calimero_context::test_support::account_for(&joiner),
        expected_group_state_hash: [0u8; 32],
        expected_context_state_hashes: Vec::new(),
    };
    MembershipRepository::new(&store)
        .remove_member(&ns, &calimero_context::test_support::account_for(&joiner))
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
        .is_member(
            &subgroup,
            &calimero_context::test_support::account_for(&joiner)
        )
        .unwrap());
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &joiner, &[[0xB3; 32]]),
        Some(false)
    );

    // REJOIN ns via a FRESHLY ISSUED invitation (direct root membership again).
    // The nonce differs from the one they joined with above, and it has to: an
    // invitation is spent for the identity that used it, so presenting the same
    // one again after exiting cannot readmit them. Coming back means being
    // re-invited.
    let rejoin_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        3,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x43; 32]),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &rejoin_ns).unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &rejoin_ns,
        None,
        [0xB4; 32],
        hlc(4),
        &[[0xB3; 32]],
    ));

    // After rejoin: inherited subgroup access is restored — both authorities.
    let live = MembershipRepository::new(&store)
        .is_member(
            &subgroup,
            &calimero_context::test_support::account_for(&joiner),
        )
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

    // Enrolled at the ROOT: bindings live at the anchor and every reader
    // resolves up to it, so a row written against the subgroup would be
    // invisible the moment it is nested.
    let admin_account = calimero_context::test_support::enrol(&store, &ns, &admin);
    for g in [&ns, &subgroup] {
        MetaRepository::new(&store)
            .save(g, &meta(admin_account))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin_account, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
        .unwrap();

    // LIVE applies the full chain — root join + inherited subgroup join — so the
    // live resolver authoritatively sees the joiner as an inherited member.
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x42; 32]),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: calimero_context::test_support::account_for(&joiner),
            group_id: subgroup.to_bytes().into(),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_sub).unwrap();
    assert!(
        MembershipRepository::new(&store)
            .is_member(
                &subgroup,
                &calimero_context::test_support::account_for(&joiner)
            )
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

/// The governance-pending drain authorizes a buffered delta with
/// `member_at_cut_authoritative` against the in-memory projection. That
/// projection can lag the durable op-store: the apply path folds an op into the
/// projection only on a real `Applied`, not on the "already applied" dedup path,
/// and ops arriving via the namespace-governance backfill pull take that dedup
/// path. So the author's membership op can be durably present in the op-store
/// yet absent from the projection — the drain then reads `None` ("cut ancestry
/// not fully folded") on every pass and eventually drops a valid delta.
///
/// The fix refreshes the projection from the op-store at the cut before
/// authorizing (`refresh_projection_for_cut`, the same step the gossip path runs
/// via `resolve_cut_membership`). This test pins the mechanism that refresh
/// relies on: with the author's root-membership ancestor missing from the
/// projection the authoritative resolver abstains, and folding that ancestor —
/// exactly what a refresh-from-store does — flips the verdict to a grant.
#[test]
fn refreshing_the_missing_ancestor_unblocks_the_authoritative_grant() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();

    let ns = ContextGroupId::from([0x51; 32]);
    let subgroup = ContextGroupId::from([0x52; 32]);

    // Genesis base state (store seeds, as `create_group` writes).
    // Enrolled at the ROOT: bindings live at the anchor and every reader
    // resolves up to it, so a row written against the subgroup would be
    // invisible the moment it is nested.
    let admin_account = calimero_context::test_support::enrol(&store, &ns, &admin);
    for g in [&ns, &subgroup] {
        MetaRepository::new(&store)
            .save(g, &meta(admin_account))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin_account, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
        .unwrap();

    // Both join ops are durably applied to the op-store — the state a node holds
    // after the backfill pull delivered them.
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x42; 32]),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: calimero_context::test_support::account_for(&joiner),
            group_id: subgroup.to_bytes().into(),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_sub).unwrap();

    let id_root_join = [0x5A; 32];
    let id_sub_join = [0x5B; 32];

    // Stale projection: the subgroup structure and the joiner's subgroup-join are
    // folded, but the joiner's ROOT-membership ancestor (`id_root_join`) is not —
    // the drain's view when the backfill applied the ops via the dedup path.
    let mut proj = ScopeProjections::new();
    let s2 = fold_subgroup_structure(
        &mut proj,
        ns.to_bytes(),
        admin,
        subgroup,
        [0x50; 32],
        [0x5F; 32],
    );
    proj.ingest_op(&op_from_namespace_op(
        &join_sub,
        None,
        id_sub_join,
        hlc(2),
        &[id_root_join], // parent is the root join — deliberately not folded yet
    ));

    // The drain's authorization fails: the cut's ancestry is incomplete, so it
    // abstains and re-buffers — even though `join_ns` is durable in the store.
    assert_eq!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[id_sub_join]),
        None,
        "stale projection: authoritative resolver must abstain while the ancestor is unfolded"
    );

    // Refresh folds the missing ancestor from the op-store (what
    // `refresh_projection_for_cut` does before the drain authorizes).
    proj.ingest_op(&op_from_namespace_op(
        &join_ns,
        None,
        id_root_join,
        hlc(1),
        &[s2],
    ));

    // Now the cut's ancestry is complete and the inherited membership resolves —
    // the drain authorizes and re-applies the delta instead of dropping it.
    assert_eq!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[id_sub_join]),
        Some(true),
        "after refreshing the durable ancestor, the authoritative resolver must grant"
    );
}

/// A credential that VERIFIES and is certified for `sign_pk`, so folding it
/// actually inserts a device. The filler fixture above cannot: `fold_device_link`
/// checks the certificate, so a `[0x11; 64]` signature is dropped before any
/// device lands.
fn real_join_account_for(
    sign_pk: &PublicKey,
    device: [u8; 32],
) -> Box<calimero_context_client::local_governance::JoinAccountCredential> {
    let root_sk = PrivateKey::random(&mut OsRng);
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
    let cert = calimero_account::DeviceCert::sign(
        &root_sk,
        genesis.account_id(),
        calimero_account::DeviceId::from(device),
        sign_pk,
        &calimero_account::KemPublicKey::from([0x2B; 32]),
        0,
        0,
    )
    .expect("sign the device cert");
    Box::new(
        calimero_context_client::local_governance::JoinAccountCredential {
            genesis,
            chain: vec![],
            statement: cert,
        },
    )
}

/// Folding a joiner's DEVICE must not disturb who is a MEMBER.
///
/// The dm-subgroup-privacy shape: the namespace admin holds no direct row in an
/// Open subgroup and reaches it purely by the inheritance walk, while a joiner in
/// the same namespace carries a credential that folds. If the device half
/// perturbs the membership half — through `account_for_author`'s precedence, the
/// enumeration, or the walk — the admin stops resolving as a member and every
/// projection-backed membership read for that subgroup denies.
#[test]
fn a_folded_join_device_does_not_hide_an_inherited_admin() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();

    let ns = ContextGroupId::from([0x31; 32]);
    let subgroup = ContextGroupId::from([0x32; 32]);

    // The admin is a direct member of the ROOT ONLY — its subgroup access is
    // inherited, exactly like the namespace owner in the scenario.
    //
    // Keyed by the admin's REAL account, and its device link folded, because
    // that is what production looks like: `admin_identity` is a real account at
    // every site that writes it, and a founder reaches the view through the
    // credential its `NamespaceCreated` carries. This test used to seed BOTH
    // sides as key-derived stand-ins, so the out-of-band root matched what a
    // bare key resolved to — the two agreed only because they were the same
    // derivation, which no production namespace reproduces.
    let admin_credential = real_join_account_for(&admin, [0x6C; 32]);
    let admin_account = admin_credential.statement.account;
    MetaRepository::new(&store)
        .save(&ns, &meta(admin_account))
        .unwrap();
    MetaRepository::new(&store)
        .save(&subgroup, &meta(admin_account))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns, &admin_account, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
        .unwrap();

    let mut proj = ScopeProjections::new();
    // The SUBGROUP IS CREATED BY THE JOINER, not the admin — as in the scenario,
    // where node-2 creates the Open subgroup. That matters: the creator becomes
    // the folded `group_admin`, so the namespace admin ends up with NO folded
    // presence at all and is reachable only through the out-of-band `root`.
    let s2 = fold_subgroup_structure(
        &mut proj,
        ns.to_bytes(),
        joiner,
        subgroup,
        [0xB0; 32],
        [0xBF; 32],
    );

    // The admin's credential folds at the ROOT, and nowhere near the subgroup.
    //
    // A key only becomes attributable once some op binds it, and the binding
    // lives in the namespace-wide view — so an admin needs a credential folded
    // SOMEWHERE in the namespace, not in the child. Folding it at the root is
    // what production looks like and it keeps the case honest: the admin still
    // has no folded presence in the subgroup, so reaching it can only be the
    // inheritance walk. Folding an open-join into the subgroup instead would
    // hand the admin a direct presence there and prove something easier.
    let admin_join_root = SignedNamespaceOp::sign(
        &admin_sk,
        ns.to_bytes().into(),
        vec![],
        7,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: admin_account,
            signed_invitation: sign_invitation(&admin_sk, ns, 2, [0x77; 32]),
            account: admin_credential,
        }),
    )
    .expect("sign admin root-join");
    let id0 = [0xB7; 32];
    proj.ingest_op(&op_from_namespace_op(
        &admin_join_root,
        None,
        id0,
        hlc(1),
        &[s2],
    ));

    // #3489's criterion, exactly: a namespace admin with a REAL enrolled account
    // inherits into an Open subgroup created by another member, holding no folded
    // presence in that subgroup at all.
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &admin, &[id0]),
        Some(true),
        "baseline: the root admin reaches an Open child by inheritance"
    );
    assert!(
        !proj
            .acl_view_at(&ScopeId::from(ns.to_bytes()), &[id0])
            .expect("scope fed")
            .groups
            .get(&subgroup)
            .is_some_and(|m| m.contains_key(&admin_account)),
        "and it must reach it WITHOUT a folded row in the subgroup, or the \
         assertion above would be proving direct membership"
    );

    // The joiner joins the namespace carrying a credential that really folds.
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x42; 32]),
            account: real_join_account_for(&joiner, [0x3E; 32]),
        }),
    )
    .expect("sign join_ns");
    let id1 = [0xB1; 32];
    proj.ingest_op(&op_from_namespace_op(&join_ns, None, id1, hlc(1), &[id0]));

    // ...and then into the Open subgroup by inheritance, credential and all.
    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: calimero_context::test_support::account_for(&joiner),
            group_id: subgroup.to_bytes().into(),
            account: real_join_account_for(&joiner, [0x3F; 32]),
        }),
    )
    .expect("sign join_sub");
    let id2 = [0xB2; 32];
    proj.ingest_op(&op_from_namespace_op(&join_sub, None, id2, hlc(2), &[id1]));

    assert_eq!(
        proj.member_at_cut(&store, subgroup, &admin, &[id2]),
        Some(true),
        "the admin's inherited membership must survive another member's device \
         folding — this is what dm-subgroup-privacy waits on"
    );
    assert_eq!(
        proj.member_at_cut(&store, ns, &admin, &[id2]),
        Some(true),
        "and its direct root membership too"
    );
}

/// The founder must be an admin AT THE CUT on a node that has only synced.
///
/// A receiver learns everything from the ops it applies. Genesis is the only op
/// that names the founder, so if the projection cannot answer "yes" here, every
/// op the founder later signs is refused on every peer while the founder's own
/// node accepts them all — the exact split the e2e cascade scenarios hit, where
/// node 1 applied the upgrade and node 2 stayed on the old schema forever.
#[test]
fn the_founder_is_admin_at_the_cut_on_a_node_that_only_synced_genesis() {
    let store = store();
    let founder_sk = PrivateKey::random(&mut OsRng);
    let founder_key = founder_sk.public_key();
    let ns = ContextGroupId::from(*founder_key);

    let credential = calimero_context::test_support::credential(&founder_key);
    let genesis = NamespaceOp::Root(RootOp::NamespaceCreated {
        founder: credential.statement.account,
        account: credential,
    });
    let signed = SignedNamespaceOp::sign(&founder_sk, (*founder_key).into(), vec![], 0, genesis)
        .expect("sign genesis");

    // Live half: exactly what a syncing receiver does with the op.
    calimero_governance_store::NamespaceGovernance::new(&store, (*founder_key).into())
        .apply_signed_op(&signed)
        .expect("receiver applies genesis");

    // Projection half: the same op folded, which is what the at-cut gate reads.
    let mut proj = ScopeProjections::new();
    let genesis_id = [0xE0u8; 32];
    proj.ingest_op(&op_from_namespace_op(
        &signed,
        None,
        genesis_id,
        hlc(0),
        &[],
    ));

    assert_eq!(
        proj.is_admin_at_cut(&store, ns, &founder_key, &[genesis_id]),
        Some(true),
        "the founder's own key must resolve to its account and be admin at the \
         genesis cut — otherwise peers refuse everything it signs"
    );

    // ...and it must STAY true once the founder creates a subgroup. This is the
    // cut every later op cites, so an answer that only holds at genesis is an
    // answer that holds nowhere in practice.
    let subgroup = ContextGroupId::from([0xE1u8; 32]);
    let created = SignedNamespaceOp::sign(
        &founder_sk,
        (*founder_key).into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            admin: calimero_account::AccountId::from([0x5C; 32]),
            group_id: subgroup.to_bytes().into(),
            parent_id: (*founder_key).into(),
            restricted: true,
        }),
    )
    .expect("sign GroupCreated");
    let created_id = [0xE1u8; 32];
    proj.ingest_op(&op_from_namespace_op(
        &created,
        None,
        created_id,
        hlc(1),
        &[genesis_id],
    ));

    assert_eq!(
        proj.is_admin_at_cut(&store, ns, &founder_key, &[created_id]),
        Some(true),
        "the founder must still be admin of the ROOT after creating a subgroup"
    );
}

/// The two planes must agree about who a key speaks for — at the same cut.
///
/// This is the invariant the whole account flip rests on: the live plane resolves
/// a signer through the binding rows, the projection resolves it through the
/// folded device links, and an op is authorized by whichever one answers. When
/// they disagree the publisher accepts an op its peers refuse, and `scope_root`
/// parts company with nothing able to reconcile it.
///
/// Exercised across the sequence that actually broke it: genesis, then a
/// subgroup creation, then a second subgroup — because the regression only
/// appeared once a `SubgroupCreated` had folded an admin under a key-derived id.
#[test]
fn both_planes_resolve_the_founder_identically_at_every_cut() {
    let store = store();
    let founder_sk = PrivateKey::random(&mut OsRng);
    let founder_key = founder_sk.public_key();
    let ns = ContextGroupId::from(*founder_key);

    let credential = calimero_context::test_support::credential(&founder_key);
    let founder_account = credential.statement.account;
    let signed_genesis = SignedNamespaceOp::sign(
        &founder_sk,
        (*founder_key).into(),
        vec![],
        0,
        NamespaceOp::Root(RootOp::NamespaceCreated {
            founder: founder_account,
            account: credential,
        }),
    )
    .expect("sign genesis");
    calimero_governance_store::NamespaceGovernance::new(&store, (*founder_key).into())
        .apply_signed_op(&signed_genesis)
        .expect("apply genesis");

    let mut proj = ScopeProjections::new();
    let mut cut = [0xF0u8; 32];
    proj.ingest_op(&op_from_namespace_op(
        &signed_genesis,
        None,
        cut,
        hlc(0),
        &[],
    ));

    for (nonce, sub) in [(1u64, [0xF1u8; 32]), (2, [0xF2u8; 32])] {
        // Live plane: the resolver every gate in `calimero-governance-store` uses.
        let live =
            calimero_governance_store::member_account_in_namespace(&store, &ns, &founder_key)
                .expect("live resolve");
        assert_eq!(
            live,
            Some(founder_account),
            "the live plane must resolve the founder to its credential's account"
        );
        // Projection plane: the at-cut gate every receiver uses.
        assert_eq!(
            proj.is_admin_at_cut(&store, ns, &founder_key, &[cut]),
            Some(true),
            "and the projection must agree the founder is admin at this cut"
        );

        let created = SignedNamespaceOp::sign(
            &founder_sk,
            (*founder_key).into(),
            vec![],
            nonce,
            NamespaceOp::Root(RootOp::GroupCreated {
                admin: calimero_account::AccountId::from([0x5C; 32]),
                group_id: ContextGroupId::from(sub).to_bytes().into(),
                parent_id: (*founder_key).into(),
                restricted: true,
            }),
        )
        .expect("sign GroupCreated");
        let next = sub;
        proj.ingest_op(&op_from_namespace_op(
            &created,
            None,
            next,
            hlc(nonce),
            &[cut],
        ));
        cut = next;
    }

    // ...and still after the last one.
    assert_eq!(
        proj.is_admin_at_cut(&store, ns, &founder_key, &[cut]),
        Some(true),
        "a subgroup creation must never cost the founder its own admin authority"
    );
}

/// An explicit device binding must outrank the key-derived stand-in, even when
/// the view carries authority under the stand-in.
///
/// Pins the precedence directly rather than through a scenario, because the
/// scenario only exposed it by accident: `SubgroupCreated` folds its admin as a
/// key-derived id, and while that id won, one subgroup creation was enough to
/// make the founder resolve to a principal no account-keyed row knows. Anything
/// that restores the old order — including deleting the legacy stand-in
/// carelessly — fails here rather than three e2e suites later.
#[test]
fn an_explicit_binding_outranks_the_key_derived_stand_in() {
    let store = store();
    let founder_sk = PrivateKey::random(&mut OsRng);
    let founder_key = founder_sk.public_key();
    let ns = ContextGroupId::from(*founder_key);

    let credential = calimero_context::test_support::credential(&founder_key);
    let founder_account = credential.statement.account;
    // The two ids for one key. They are different by construction, which is the
    // whole reason the precedence matters.
    assert_ne!(
        founder_account,
        calimero_op::Authorship::UNATTRIBUTED_ACCOUNT,
        "the derived stand-in is not the account a credential certifies"
    );

    let signed = SignedNamespaceOp::sign(
        &founder_sk,
        (*founder_key).into(),
        vec![],
        0,
        NamespaceOp::Root(RootOp::NamespaceCreated {
            founder: founder_account,
            account: credential,
        }),
    )
    .expect("sign genesis");
    calimero_governance_store::NamespaceGovernance::new(&store, (*founder_key).into())
        .apply_signed_op(&signed)
        .expect("apply genesis");

    let mut proj = ScopeProjections::new();
    let genesis_id = [0xD0u8; 32];
    proj.ingest_op(&op_from_namespace_op(
        &signed,
        None,
        genesis_id,
        hlc(0),
        &[],
    ));

    // A GroupCreated used to fold `admin` as a stand-in for the signer — authority in the
    // view under the STAND-IN, which is exactly the condition that used to make
    // the stand-in win.
    let created = SignedNamespaceOp::sign(
        &founder_sk,
        (*founder_key).into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            admin: calimero_account::AccountId::from([0x5C; 32]),
            group_id: ContextGroupId::from([0xD1u8; 32]).to_bytes().into(),
            parent_id: (*founder_key).into(),
            restricted: true,
        }),
    )
    .expect("sign GroupCreated");
    let created_id = [0xD1u8; 32];
    proj.ingest_op(&op_from_namespace_op(
        &created,
        None,
        created_id,
        hlc(1),
        &[genesis_id],
    ));

    assert_eq!(
        proj.is_admin_at_cut(&store, ns, &founder_key, &[created_id]),
        Some(true),
        "the binding must still decide who this key is, or the founder resolves \
         to a principal the account-keyed rows have never heard of"
    );
}

/// **A join is attributed to the account its certificate names, not to a
/// stand-in derived from its signing key.**
///
/// This is the property the whole bridge deletion turns on. While
/// `op_from_namespace_op` synthesised authorship with
/// `calimero_op::Authorship::unattributed(signer)`, a join op's `device` was a reinterpretation of
/// the derived account's bytes — a device that was never enrolled and holds no
/// key. `calimero_authz::authorize` says so at its `MemberJoinedWithDevice`
/// arm, where two cross-checks its `DeviceLinked` sibling runs are documented
/// as impossible precisely because "both the device-key and the account
/// comparison would fail on a perfectly honest join".
///
/// So this asserts the three things those checks need: the op is attributed to
/// the certified account, to the certified device, and to the key that signed
/// it. Assert the stand-in is a *different* account too — otherwise the first
/// assertion could pass for the wrong reason.
#[test]
fn a_join_is_attributed_to_the_account_its_certificate_names() {
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();
    let ns = ContextGroupId::from([0x21; 32]);
    let group = ContextGroupId::from([0x22; 32]);

    let credential = real_join_account_for(&joiner, [0x4D; 32]);
    let certified_account = credential.statement.account;
    let certified_device = credential.statement.device;

    let join = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: certified_account,
            group_id: group.to_bytes().into(),
            account: credential,
        }),
    )
    .expect("sign the join");

    let op = op_from_namespace_op(&join, None, [0x9C; 32], hlc(1), &[]);

    assert_eq!(
        op.author(),
        certified_account,
        "the op must name the account the certificate grants to, since that is \
         what `authorize` compares its verified account against"
    );
    assert_eq!(
        op.device(),
        certified_device,
        "and the enrolled device, which is the CRDT replica slot — a fabricated \
         one makes every account share a single slot"
    );
    assert_eq!(
        *op.device_key(),
        joiner,
        "and the key that actually signed it, so possession can be required"
    );
    assert_ne!(
        certified_account,
        calimero_op::Authorship::UNATTRIBUTED_ACCOUNT,
        "the stand-in must differ from the certified account, or the assertions \
         above would hold no matter which one was used"
    );
}

/// **A key rotation authored by an enrolled device must actually absorb.**
///
/// The fold gates `AccountKeysRotated` on `handoff.account == op.authorship.account`
/// — deliberately, because `from_ops` and the sync convergence path fold raw logs
/// without `authorize`, and that check is what stops a stranger absorbing into a
/// victim's epoch slot.
///
/// But `GroupOp::AccountKeysRotated` carries no credential, so the production
/// converter has nothing to attribute the op to and stamps
/// `calimero_op::Authorship::unattributed(signer)` — a stand-in derived from the signing key. A
/// stand-in never equals the real `handoff.account`, so the comparison fails on
/// an honest rotation and the handoff is dropped.
///
/// `crates/projection/tests/account_plane.rs` misses this because its fixture
/// builds `Authorship` from a device that knows its own account, supplying the
/// truthful value production fabricates. This drives the real converter instead.
#[test]
fn a_rotation_by_an_enrolled_device_absorbs_through_the_real_converter() {
    let ns = ContextGroupId::from([0x51; 32]);
    let group = ContextGroupId::from([0x52; 32]);

    // An account with a real genesis, and a device certified by its root key.
    let root_sk = PrivateKey::from([0x61u8; 32]);
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
    let account = genesis.account_id();
    let device_sk = PrivateKey::from([0x62u8; 32]);
    let device_key = device_sk.public_key();
    let cert = calimero_account::DeviceCert::sign(
        &root_sk,
        account,
        calimero_account::DeviceId::from([0x63; 32]),
        &device_key,
        &calimero_account::KemPublicKey::from([0x64; 32]),
        0,
        0,
    )
    .expect("sign the device cert");

    let mut proj = ScopeProjections::new();

    // The device links, so the binding is folded and the account is known.
    let link_env = ns_group_envelope(ns.to_bytes(), device_key, group);
    let link = GroupOp::AccountDeviceLinked {
        genesis,
        chain: vec![],
        cert,
        endorsement: calimero_account::AccountMemberEndorsement::sign(&device_sk, account)
            .expect("endorse"),
    };
    let link_id = [0xC1; 32];
    proj.ingest_op(&op_from_namespace_op(
        &link_env,
        Some(&link),
        link_id,
        hlc(1),
        &[],
    ));

    // That device now rotates its account's root key.
    let handoff = calimero_account::RootKeyHandoff::sign(
        &root_sk,
        account,
        0,
        &PrivateKey::from([0x65u8; 32]).public_key(),
    )
    .expect("sign handoff");
    let rot_env = ns_group_envelope(ns.to_bytes(), device_key, group);
    let rotation = GroupOp::AccountKeysRotated { handoff };
    let rot_id = [0xC2; 32];
    // The production path: the caller resolves the signer's binding and passes
    // it, exactly as the apply handler and the backfill now do.
    let rot_op = calimero_governance_store::op_from_namespace_op_with_binding(
        &rot_env,
        Some(&rotation),
        Some((account, cert.device)),
        rot_id,
        hlc(2),
        &[link_id],
    );

    // Told who signed, the converter names the real account.
    assert_eq!(
        rot_op.author(),
        account,
        "the op must be attributed to the rotating account, not to a stand-in"
    );
    // And the stand-in it would otherwise have used is a different account — so
    // the assertion below cannot pass by coincidence.
    assert_ne!(
        calimero_op::Authorship::UNATTRIBUTED_ACCOUNT,
        account,
        "precondition: the derived stand-in differs from the real account"
    );
    let unattributed =
        op_from_namespace_op(&rot_env, Some(&rotation), [0xC3; 32], hlc(2), &[link_id]);
    assert_ne!(
        unattributed.author(),
        account,
        "precondition: without the binding the converter still stands in, which is \
         what silently dropped every rotation"
    );

    proj.ingest_op(&rot_op);

    let view = proj
        .acl_view_at(&ScopeId::from(ns.to_bytes()), &[rot_id])
        .expect("scope fed");
    assert_eq!(
        view.accounts.get(&account).map(|a| a.epoch),
        Some(1),
        "the rotation must have been absorbed: the account's epoch should have \
         advanced to 1. If this is 0 or None, the fold compared the handoff's \
         real account against the converter's stand-in and dropped the rotation."
    );
}

/// The CI failure this gate produced, reduced: an author promotes a member, then
/// its OWN earlier add lands in the DAG afterwards (the publisher path writes
/// live directly, so an author's ops reach its DAG only through the apply feed).
/// The projection answers about the add's cut — `Member`, correctly, the
/// promotion is not an ancestor — while live answers about now — `Admin`, also
/// correctly. Neither plane is wrong, so the shadow must not call it a
/// divergence; the frontier gate (`has_folded_all` over live's governance head)
/// is what tells the two situations apart.
#[test]
fn a_late_applied_add_after_a_promotion_is_not_a_divergence() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let member_sk = PrivateKey::random(&mut OsRng);
    let member = member_sk.public_key();

    let ns = ContextGroupId::from([0x31; 32]);
    let subgroup = ContextGroupId::from([0x32; 32]);
    let scope = ScopeId::from(ns.to_bytes());

    // Genesis seeds, as `create_group` writes them: both groups exist, the admin
    // is Admin of both, the subgroup is nested under the root.
    let admin_account = calimero_context::test_support::enrol(&store, &ns, &admin);
    let member_account = calimero_context::test_support::enrol(&store, &ns, &member);
    for g in [&ns, &subgroup] {
        MetaRepository::new(&store)
            .save(g, &meta(admin_account))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin_account, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();

    let add = GroupOp::MemberAdded {
        member: member_account,
        role: GroupMemberRole::Member,
    };
    let promote = GroupOp::MemberRoleSet {
        member: member_account,
        role: GroupMemberRole::Admin,
    };
    let add_id = [0xD1; 32];
    let promote_id = [0xD2; 32];

    // The publisher path, twice: each op mutates the live store and advances the
    // persisted governance head, and neither touches the DAG or the projection.
    // (`advance_dag_head` + the live write is exactly what `publish_post_gate`
    // does; the encrypted group op cannot go through `apply_signed_namespace_op`
    // here because that needs the group key.)
    let gov = calimero_governance_store::NamespaceDagService::new(&store, ns.to_bytes().into());
    MembershipRepository::new(&store)
        .add_member(&subgroup, &member_account, GroupMemberRole::Member)
        .unwrap();
    gov.advance_dag_head(add_id, &[], 1).unwrap();
    MembershipRepository::new(&store)
        .set_role(&subgroup, &member_account, GroupMemberRole::Admin)
        .unwrap();
    gov.advance_dag_head(promote_id, &[add_id], 2).unwrap();

    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&subgroup, &member_account)
            .unwrap(),
        Some(GroupMemberRole::Admin),
        "live holds the promotion — it applied at publish time",
    );

    // Now the author's own ADD arrives through the apply feed, after the
    // promotion has already published. This is the only op the projection has.
    let mut proj = ScopeProjections::new();
    let env = ns_group_envelope(ns.to_bytes(), admin, subgroup);
    proj.ingest_op(&op_from_namespace_op(&env, Some(&add), add_id, hlc(1), &[]));

    let live_heads = gov.read_head_record().unwrap().parent_hashes;
    assert_eq!(
        live_heads,
        vec![promote_id],
        "live's frontier is the promotion, the newest op it applied",
    );
    assert_eq!(
        proj.role_at_cut(&scope, &subgroup, &member_account, &[add_id]),
        Some(GroupMemberRole::Member),
        "at the add's own cut the member IS a Member — the promotion comes after it",
    );
    assert!(
        !proj.cut_covers_frontier(&scope, &[add_id], &live_heads),
        "the add's cut does not reach the promotion live already applied, so its \
         at-cut answer and live's row are answers to different questions and the \
         shadow compare must stay quiet",
    );

    // The promotion then folds too — through the same feed, one op later — and
    // the compare it triggers runs at a cut live has caught up with.
    proj.ingest_op(&op_from_namespace_op(
        &env,
        Some(&promote),
        promote_id,
        hlc(2),
        &[add_id],
    ));
    let live_heads = gov.read_head_record().unwrap().parent_hashes;
    assert!(
        proj.cut_covers_frontier(&scope, &[promote_id], &live_heads),
        "the promotion's own cut reaches live's frontier — it IS live's frontier — \
         so the compare its apply triggers is a real one",
    );
    // And the add's cut is STILL not comparable, now that the log holds both ops:
    // the gate is the cut, not how much has been folded. A log-completeness check
    // would have re-opened the false positive here.
    assert!(
        !proj.cut_covers_frontier(&scope, &[add_id], &live_heads),
        "folding the promotion cannot put it inside a cut that precedes it",
    );
    assert_eq!(
        proj.role_at_cut(&scope, &subgroup, &member_account, &[promote_id]),
        MembershipRepository::new(&store)
            .role_of(&subgroup, &member_account)
            .unwrap(),
        "and then they agree: Admin at the promotion's cut, Admin in the live row",
    );
}

/// The authoritative grant path must abstain when an ancestor is UNREADABLE, not
/// only when one is missing.
///
/// A gap and a hole cost the same over-grant by different routes. With a gap, the
/// removal op is absent and the walk still sees the member; with a hole, the
/// removal op is present but encrypted under a key this node does not hold, folds
/// to nothing, and the walk still sees the member. The view is equally stale, and
/// this path GRANTS on what it returns — so it defers to live in both cases.
///
/// Same reasoning covers the apply gates through `auth_cut_context`, where the
/// verdict is worse than stale: `PermissionChecker` returns a `Some(false)`
/// straight through as a refusal, so two nodes with different key epochs would
/// decide one op differently and that namespace's governance DAG would stop
/// converging on the one that refused.
#[test]
fn the_grant_path_defers_when_an_ancestor_is_unreadable() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();

    let ns = ContextGroupId::from([0x61; 32]);
    let subgroup = ContextGroupId::from([0x62; 32]);

    let admin_account = calimero_context::test_support::enrol(&store, &ns, &admin);
    for g in [&ns, &subgroup] {
        MetaRepository::new(&store)
            .save(g, &meta(admin_account))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin_account, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();

    let mut proj = ScopeProjections::new();
    let s2 = fold_subgroup_structure(
        &mut proj,
        ns.to_bytes(),
        admin,
        subgroup,
        [0xD0; 32],
        [0xDF; 32],
    );

    // The joiner is a member at this cut, by an op the projection HAS folded.
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, subgroup, 1, [0x42; 32]),
            account: test_join_account_for(&joiner),
        }),
    )
    .expect("sign join");
    calimero_governance_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    let joined = [0xD1; 32];
    proj.ingest_op(&op_from_namespace_op(&join_ns, None, joined, hlc(1), &[s2]));

    // Complete and readable: the grant path answers.
    assert_eq!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[joined]),
        Some(true),
        "precondition: a whole, readable ancestry is answerable",
    );

    // Now a descendant op this node cannot decrypt — a group op with no key here.
    // Its payload folds to nothing, so the view is unchanged and the member still
    // looks present; what it could have said (a removal, a role change) is exactly
    // what this node cannot know.
    let hole = op_from_namespace_op(
        &ns_group_envelope(ns.to_bytes(), admin, subgroup),
        None,
        [0xD2; 32],
        hlc(2),
        &[joined],
    );
    proj.ingest_op(&hole);

    assert_eq!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[hole.id()]),
        None,
        "an unreadable ancestor must make the grant path DEFER to live, not grant \
         from a fold that is missing whatever that op did",
    );
    // The narrower shadow question still answers — it is asked about direct rows
    // only, where a sibling group's hole cannot matter. The two coexisting is the
    // point: strict where a verdict is authoritative, narrow where it is a
    // comparison.
    assert_eq!(
        proj.cut_ancestry_state(&ScopeId::from(ns.to_bytes()), &[hole.id()]),
        (true, false),
        "complete but unreadable, which is the state under test",
    );
}
